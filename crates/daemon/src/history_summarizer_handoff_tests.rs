//! The History Summarizer's reserve, stage, activate handoff against a real Kernel and Memory Store (KTD3, #595): the reservation exists before anything is staged and is recorded in the producer's durable state; staging and sealing converge after an interruption at every boundary; the fenced publication activates the job with its history or commits neither; an expired reservation is closed as expired by a late publication; identical inputs neither duplicate a job nor reopen a settled one; capacity refusal before the reservation is the nonadmission the publication records; and the staged subject reports one origin per cited block with exact ranges.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::history_summarizer::{ValidatedPublishRequest, publish_validated_chunk};
use crate::history_summarizer_citations::{Citation, FrozenAlias, FrozenAliasTable};
use crate::history_summarizer_validate::{
    FactCandidate, UserObservationCandidate, ValidatedChunk, ValidatedHistorySegment,
};
use crate::memory_reviewer::handoff::{
    Handoff, HandoffError, HandoffRequest, HandoffTarget, PRODUCER, SUBJECT_SOURCE_KIND,
    handoff_key, reserve_and_stage, review_binding, review_policy_versions, review_subject,
};
use kernel::{
    ByteRange, KernelStore, ReviewPayload, ReviewReadError, ReviewReadRefusal,
    ReviewStagedReference,
};
use memory_store::HistorySummarizerPublishPredicate;
use memory_store::memory_reviewer_jobs::{
    CausalInputs, MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT, MEMORY_REVIEWER_QUEUE_LIFETIME_MS,
    MemoryReviewerJobOutcome, MemoryReviewerJobState, ProducerBinding, ReviewTarget,
};
use memory_store::{
    BlockIdentity, HistorySegmentSetGeneration, HistorySummarizerChunkRange,
    HistorySummarizerDurableState, HistorySummarizerPhase, HistorySummarizerPublishError,
    HistorySummarizerSelectedMessageIdentity, MemoryReviewerActivationOutcome,
    MemoryReviewerNonadmissionCode, MemoryStore, ModuleMeta,
};

const PROJECT: &str = "git:proj";
const PROJECT_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DOMAIN: &str = "memory";
const SESSION: &str = "ses";
/// The Kernel judges staging deadlines against the wall clock, so the rig's origin is the real present, fixed once per process.
fn t0() -> i64 {
    static ORIGIN: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *ORIGIN.get_or_init(crate::now_ms)
}

struct Rig {
    _dirs: Vec<tempfile::TempDir>,
    kernel: Arc<KernelStore>,
    store: MemoryStore,
    kernel_incarnation: String,
}

impl Rig {
    fn open() -> Self {
        let kernel_dir = tempfile::tempdir().unwrap();
        let kernel = Arc::new(KernelStore::open(kernel_dir.path()).unwrap());
        let budget = kernel::applicability::EvalBudget::new(None, Arc::default());
        let kernel_incarnation = kernel
            .database_incarnation_id_within_budget(&budget)
            .unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&MemoryStore::test_descriptor(
            store_dir.path(),
            "eidnara-history-summarizer-handoff-test",
        ))
        .unwrap();
        let rig = Rig {
            _dirs: vec![kernel_dir, store_dir],
            kernel,
            store,
            kernel_incarnation,
        };
        // The session is in Publishing for firing 3 over messages 2..=4 with its selected identities recorded, as the live path leaves it before publication.
        let mut meta = ModuleMeta::default();
        for selected in selected_range_identities() {
            meta.block_identity_by_mid
                .insert(selected.mid, selected.block_identities);
        }
        meta.history_summarizer = publishing_state(3);
        rig.store
            .commit(SESSION, None, &cache_stability::CoreState::empty(), &meta)
            .unwrap();
        rig
    }

    fn target(&self) -> HandoffTarget {
        HandoffTarget {
            kernel: Arc::clone(&self.kernel),
            project_digest: PROJECT_DIGEST.to_string(),
            domain_id: DOMAIN.to_string(),
            kernel_incarnation: self.kernel_incarnation.clone(),
        }
    }

    fn state(&self) -> HistorySummarizerDurableState {
        self.store.load(SESSION).unwrap().meta.history_summarizer
    }

    /// The reservation the durable state records for the current firing.
    fn reservation(&self) -> memory_store::MemoryReviewerReservation {
        self.state()
            .memory_reviewer_reservation
            .expect("the reservation is recorded")
    }

    fn persist(&self, state: HistorySummarizerDurableState) -> u64 {
        let loaded = self.store.load(SESSION).unwrap();
        let mut meta = loaded.meta.clone();
        meta.history_summarizer = state;
        self.store
            .commit(SESSION, loaded.row_version, &loaded.core, &meta)
            .unwrap()
    }

    /// The handoff as the publication path runs it: the reservation is written into the Publishing state the moment it exists.
    fn handoff(&self, now_ms: i64) -> Result<Handoff, HandoffError> {
        self.handoff_observing(now_ms, |_| {})
    }

    /// `observe` runs inside the persistence step, after the job row exists and before anything is staged.
    fn handoff_observing(
        &self,
        now_ms: i64,
        observe: impl FnOnce(&memory_store::MemoryReviewerReservation),
    ) -> Result<Handoff, HandoffError> {
        let firing = self.state();
        reserve_and_stage(
            &self.target(),
            &HandoffRequest {
                store: &self.store,
                project: PROJECT,
                session_id: SESSION,
                firing: &firing,
                facts: &facts(),
                aliases: &aliases(),
                now_ms,
            },
            |reservation| {
                observe(reservation);
                Ok(self.retain(reservation, &pending_publication(&validated_range(2, 4))))
            },
        )
    }

    fn retain(
        &self,
        reservation: &memory_store::MemoryReviewerReservation,
        pending: &memory_store::PendingPublication,
    ) -> u64 {
        let row_version = self.store.load(SESSION).unwrap().row_version.unwrap();
        self.store
            .record_memory_reviewer_reservation(SESSION, row_version, reservation, pending)
            .unwrap()
    }

    fn pending(&self) -> Option<(u64, memory_store::PendingPublication)> {
        self.store.load_pending_publication(SESSION).unwrap()
    }

    /// Reopens the Memory Store from its files, as a restarted daemon would; the old handle releases its lease first.
    fn reopen(&mut self) {
        let path = self._dirs[1].path().to_path_buf();
        let placeholder_dir = tempfile::tempdir().unwrap();
        let placeholder = MemoryStore::open(&MemoryStore::test_descriptor(
            placeholder_dir.path(),
            "eidnara-history-summarizer-handoff-placeholder",
        ))
        .unwrap();
        drop(std::mem::replace(&mut self.store, placeholder));
        self.store = MemoryStore::open(&MemoryStore::test_descriptor(
            &path,
            "eidnara-history-summarizer-handoff-test",
        ))
        .unwrap();
        self._dirs.push(placeholder_dir);
    }

    fn republish(&self, target: Option<&HandoffTarget>, now_ms: i64) -> RepublishOutcome {
        republish_reserved(RepublishRequest {
            store: &self.store,
            session_id: SESSION,
            project_path: PROJECT,
            memory_reviewer_handoff: target,
            now_ms,
            failure_backoff_at_ms: now_ms + 60_000,
            publication_fence: None,
            collect_user_memory_candidates: false,
            memory_enabled: true,
        })
        .unwrap()
    }

    /// The reference the handoff stages the accepted facts under.
    fn staged_reference(&self) -> ReviewStagedReference {
        let payload = ReviewPayload::Subject(review_subject(&facts(), &aliases()).unwrap());
        let payload_digest = payload.digest().unwrap();
        ReviewStagedReference {
            database_incarnation_id: self.kernel_incarnation.clone(),
            candidate_id: format!("hs-{}-{}", rig_key(), &payload_digest[..32]),
            payload_digest,
        }
    }

    fn publish(
        &self,
        activation: Option<&PreparedActivation>,
        nonadmission: Option<MemoryReviewerNonadmissionCode>,
        now_ms: i64,
    ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerStateError> {
        self.publish_range(activation, nonadmission, now_ms, 2, 4)
    }

    fn publish_range(
        &self,
        activation: Option<&PreparedActivation>,
        nonadmission: Option<MemoryReviewerNonadmissionCode>,
        now_ms: i64,
        start: u64,
        end: u64,
    ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerStateError> {
        let loaded = self.store.load(SESSION).unwrap();
        let predicate = publish_predicate(&loaded.meta.history_summarizer).unwrap();
        self.publish_at(
            loaded.row_version,
            &predicate,
            activation,
            nonadmission,
            now_ms,
            start,
            end,
        )
    }

    /// Publishes against `expected_row_version` and `predicate`, which may both be stale.
    #[allow(clippy::too_many_arguments)]
    fn publish_at(
        &self,
        expected_row_version: Option<u64>,
        predicate: &HistorySummarizerPublishPredicate,
        activation: Option<&PreparedActivation>,
        nonadmission: Option<MemoryReviewerNonadmissionCode>,
        now_ms: i64,
        start: u64,
        end: u64,
    ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerStateError> {
        let validated = validated_range(start, end);
        publish_validated_chunk(
            &self.store,
            ValidatedPublishRequest {
                session_id: SESSION,
                project_path: PROJECT,
                expected_row_version,
                expected_revert_epoch: 0,
                predicate,
                observed_chunk_fingerprint: "fp",
                validated: &validated,
                collect_user_memory_candidates: false,
                publication_floor_ordinal: end + 1,
                chunk_transcript: "U: transcript",
                boundary_dates: &BTreeMap::new(),
                created_at_ms: now_ms,
                now_ms,
                failure_backoff_at_ms: now_ms + 60_000,
                publication_fence: None,
                memory_reviewer_nonadmission: nonadmission,
                memory_reviewer_activation: activation,
            },
        )
    }

    fn job(&self, causal_identity: &str) -> memory_store::memory_reviewer_jobs::MemoryReviewerJob {
        self.store
            .lookup_memory_reviewer_job(PROJECT, causal_identity)
            .unwrap()
            .expect("the job row exists")
    }

    fn read_subject(
        &self,
        reservation: &memory_store::MemoryReviewerReservation,
        now_ms: i64,
    ) -> Result<kernel::ReviewStagedRow, ReviewReadError> {
        self.kernel.read_review_input(
            &ReviewStagedReference {
                database_incarnation_id: reservation.kernel_incarnation.clone(),
                candidate_id: reservation.candidate_id.clone(),
                payload_digest: reservation.payload_digest.clone(),
            },
            &review_binding(
                PROJECT_DIGEST,
                DOMAIN,
                SESSION,
                self.job(&reservation.causal_identity).producer.ordinal,
                &reservation.causal_identity,
            ),
            now_ms,
        )
    }
}

/// The key the rig's handoffs derive their Kernel and job identities from.
fn rig_key() -> String {
    handoff_key(PROJECT_DIGEST, SESSION, &review_policy_versions())
}

fn selected_range_identities() -> Vec<HistorySummarizerSelectedMessageIdentity> {
    vec![HistorySummarizerSelectedMessageIdentity {
        mid: "m2".to_string(),
        block_identities: vec![BlockIdentity {
            kind_tag: "text".to_string(),
            byte_fingerprint: "content-a".to_string(),
        }],
    }]
}

fn publishing_state(firing_seq: u64) -> HistorySummarizerDurableState {
    HistorySummarizerDurableState {
        state: HistorySummarizerPhase::Publishing,
        firing_seq,
        chunk_range: Some(HistorySummarizerChunkRange {
            from_ordinal: 2,
            to_ordinal: 4,
        }),
        chunk_fingerprint: "fp".to_string(),
        selected_range_identities: selected_range_identities(),
        producer_session_id: Some("producer".to_string()),
        producer_run_id: Some(format!("run-{firing_seq}")),
        producer_harness: Some("pi".to_string()),
        fired_at_ms: Some(t0()),
        ..HistorySummarizerDurableState::default()
    }
}

fn aliases() -> FrozenAliasTable {
    let mut table = FrozenAliasTable::default();
    for (ordinal, presented) in [
        (2u64, "bun install runs before every build"),
        (3, "the workspace uses bun"),
        (4, "done"),
    ] {
        table.issue(FrozenAlias {
            message_id: format!("m{ordinal}"),
            ordinal,
            block_ids: vec![format!("m{ordinal}#0"), format!("m{ordinal}#1")],
            block_hashes: vec![
                format!("{:064x}", ordinal),
                format!("{:064x}", ordinal * 100),
            ],
            presented: presented.to_string(),
            ..FrozenAlias::default()
        });
    }
    table
}

/// Two facts citing one block: the first cites `s1` twice at distinct ranges and `s2`; the second cites `s1` again at a range the first already cited.
fn facts() -> Vec<FactCandidate> {
    let citation = |alias: &str, start: usize, end: usize| Citation {
        alias: alias.to_string(),
        start,
        end,
    };
    vec![
        FactCandidate {
            category: "PROJECT_RULES".to_string(),
            content: "Run bun install before building.".to_string(),
            citations: vec![
                citation("s1", 0, 11),
                citation("s1", 12, 35),
                citation("s2", 0, 22),
            ],
        },
        FactCandidate {
            category: "CONFIG_VALUES".to_string(),
            content: "The package manager is bun.".to_string(),
            citations: vec![citation("s1", 0, 11)],
        },
    ]
}

fn validated_range(start: u64, end: u64) -> ValidatedChunk {
    ValidatedChunk {
        history_segments: vec![ValidatedHistorySegment {
            sequence: 0,
            start_message: start,
            end_message: end,
            start_message_id: format!("m{start}"),
            end_message_id: format!("m{end}"),
            title: "arc".to_string(),
            content: "arc".to_string(),
            p1: Some("arc".to_string()),
            p2: None,
            p3: None,
            p4: None,
            importance: Some(50),
            episode_type: Some("feature".to_string()),
        }],
        facts: facts(),
        unprocessed_from: end + 1,
        ..ValidatedChunk::default()
    }
}

fn pending_publication(validated: &ValidatedChunk) -> memory_store::PendingPublication {
    memory_store::PendingPublication {
        validated_json: serde_json::to_string(validated).unwrap(),
        aliases_json: serde_json::to_string(&aliases()).unwrap(),
        chunk_transcript: "U: transcript".to_string(),
        boundary_dates: BTreeMap::new(),
        publication_floor_ordinal: 5,
        collect_user_memory_candidates: false,
        created_at_ms: t0(),
    }
}

fn activation(handoff: Handoff) -> PreparedActivation {
    match handoff {
        Handoff::Activate(prepared) => *prepared,
        other => panic!("expected an activation, got {other:?}"),
    }
}

/// Drives the next firing over `from_ordinal..=to_ordinal` to Publishing through the production transitions; `fire` leaves the prior firing's reservation behind.
fn next_publishing_firing(
    rig: &Rig,
    from_ordinal: u64,
    to_ordinal: u64,
) -> HistorySummarizerDurableState {
    let idle = rig.state();
    assert_eq!(idle.state, HistorySummarizerPhase::Idle);
    let FireOutcome::Fired(firing) = fire(
        &idle,
        from_ordinal,
        to_ordinal,
        "fp".to_string(),
        selected_range_identities(),
        0,
        HistorySegmentSetGeneration::default(),
        t0(),
    )
    .unwrap() else {
        panic!("the idle state fires");
    };
    let awaiting = producer_started(
        &firing,
        "producer".to_string(),
        format!("run-{}", firing.firing_seq),
        "pi".to_string(),
    )
    .unwrap();
    let validating = output_received(&awaiting, "").unwrap();
    validation_ok(&validating).unwrap()
}

#[test]
fn a_reservation_left_by_an_abandoned_firing_does_not_block_the_next_firing() {
    let rig = Rig::open();
    let orphaned = activation(rig.handoff(t0()).unwrap());
    let stale = rig.reservation();
    // Firing 3 fails after the reservation exists and recovery abandons it with the reservation retained.
    let abandoned = abandon_with_detail(&rig.state(), t0() + 1, Some("crash".to_string()));
    assert_eq!(abandoned.memory_reviewer_reservation, Some(stale.clone()));
    rig.persist(abandoned);
    // Firing 4 summarizes the next chunk and extracts nothing to hand off; the stale reservation is not its to publish, so `fire` drops it.
    let next = next_publishing_firing(&rig, 5, 6);
    assert_eq!(next.firing_seq, 4);
    assert_eq!(next.memory_reviewer_reservation, None);
    rig.persist(next);
    let result = rig.publish_range(None, None, t0() + 10, 5, 6).unwrap();
    assert_eq!(result.memory_reviewer_activation, None);
    assert_eq!(result.memory_reviewer_nonadmission_count, 0);
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(7));
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    // The orphaned job is untouched; the expiry sweep closes it.
    assert_eq!(
        rig.job(&orphaned.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );
}

#[test]
fn identical_facts_from_the_next_firing_adopt_the_orphaned_reservation() {
    let rig = Rig::open();
    let first = activation(rig.handoff(t0()).unwrap());
    let orphaned = rig.reservation();
    rig.persist(abandon_with_detail(
        &rig.state(),
        t0() + 1,
        Some("crash".to_string()),
    ));
    // Firing 4 re-summarizes the same chunk and extracts the same facts.
    rig.persist(next_publishing_firing(&rig, 2, 4));
    let headroom_before = rig.store.memory_reviewer_headroom(PROJECT).unwrap();
    let adopted = activation(rig.handoff(t0() + 10).unwrap());
    assert_eq!(adopted.causal_identity, first.causal_identity);
    assert_eq!(adopted.producer.firing_id, format!("{}#4", rig_key()));
    assert_eq!(
        rig.store.memory_reviewer_headroom(PROJECT).unwrap(),
        headroom_before,
        "adoption reserves nothing new"
    );
    // The durable reservation now names this firing; the job's deadline and identity never moved.
    let reservation = rig.reservation();
    assert_eq!(reservation.firing_seq, 4);
    assert_eq!(reservation.causal_identity, orphaned.causal_identity);
    assert_eq!(reservation.candidate_id, orphaned.candidate_id);
    assert_eq!(reservation.queue_deadline_ms, orphaned.queue_deadline_ms);
    let job = rig.job(&first.causal_identity);
    assert_eq!(job.state, MemoryReviewerJobState::Reserved);
    assert_eq!(job.producer, adopted.producer);
    assert_eq!(job.queue_deadline_ms, orphaned.queue_deadline_ms);
    // The fenced publication activates the adopted job with this firing's history.
    let result = rig.publish(Some(&adopted), None, t0() + 11).unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Activated)
    );
    assert_eq!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, t0() + 11)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(rig.state().memory_reviewer_reservation, None);
}

#[test]
fn the_subject_reports_one_origin_per_cited_part_with_exact_ranges() {
    let subject = review_subject(&facts(), &aliases()).unwrap();
    assert_eq!(subject.facts.len(), 2);
    assert_eq!(subject.facts[0].spans.len(), 3);
    assert_eq!(subject.facts[1].spans[0].alias, "s1");
    // `s1` is cited four times across two facts at two distinct ranges; it is one origin naming the native message part's blocks and both ranges once each, and `s3` is uncited so it is absent.
    assert_eq!(subject.origins.len(), 2);
    let s1 = &subject.origins[0];
    assert_eq!(s1.alias, "s1");
    assert_eq!(s1.message_id, "m2");
    assert_eq!(s1.ordinal, 2);
    assert_eq!(s1.block_ids, ["m2#0", "m2#1"]);
    assert_eq!(
        s1.block_hashes,
        [format!("{:064x}", 2), format!("{:064x}", 200)]
    );
    assert_eq!(
        s1.ranges,
        [
            ByteRange { start: 0, end: 11 },
            ByteRange { start: 12, end: 35 }
        ]
    );
    assert_eq!(subject.origins[1].alias, "s2");
    // The Kernel accepts it as a payload, and its bytes carry no presented text beyond the facts.
    let encoded = ReviewPayload::Subject(subject).encode().unwrap();
    assert!(!encoded.contains("bun install runs before every build"));
}

#[test]
fn reservation_precedes_staging_and_publication_activates_with_progress() {
    let rig = Rig::open();
    let prepared = activation(
        rig.handoff_observing(t0(), |reservation| {
            // Inside the persistence step the job row is Reserved and nothing is staged yet.
            assert_eq!(
                rig.job(&reservation.causal_identity).state,
                MemoryReviewerJobState::Reserved
            );
            assert!(matches!(
                rig.read_subject(reservation, t0()),
                Err(ReviewReadError::Refused(ReviewReadRefusal::Missing))
            ));
        })
        .unwrap(),
    );
    // The reservation is in the durable state and the row is Reserved before the fenced publication.
    let state = rig.state();
    assert_eq!(state.state, HistorySummarizerPhase::Publishing);
    let reservation = state
        .memory_reviewer_reservation
        .clone()
        .expect("recorded before staging");
    assert_eq!(reservation.causal_identity, prepared.causal_identity);
    assert_eq!(
        Some(prepared.row_version),
        rig.store.load(SESSION).unwrap().row_version,
        "the publication's CAS expects the row the reservation was written in"
    );
    assert_eq!(reservation.firing_seq, 3);
    assert_eq!(
        reservation.queue_deadline_ms,
        t0() + MEMORY_REVIEWER_QUEUE_LIFETIME_MS
    );
    let job = rig.job(&reservation.causal_identity);
    assert_eq!(job.state, MemoryReviewerJobState::Reserved);
    assert_eq!(
        job.producer,
        ProducerBinding {
            producer: PRODUCER.to_string(),
            firing_id: format!("{}#3", rig_key()),
            ordinal: 2,
        }
    );
    assert_eq!(
        job.target,
        ReviewTarget::StagedSubject {
            kernel_incarnation: rig.kernel_incarnation.clone(),
            candidate_id: reservation.candidate_id.clone(),
            payload_digest: reservation.payload_digest.clone(),
        }
    );
    // The binding cites the session's chunk by its first message, not the firing.
    let row = rig.read_subject(&reservation, t0() + 1).unwrap();
    assert_eq!(row.binding.subject_source.source_kind, SUBJECT_SOURCE_KIND);
    assert_eq!(row.binding.subject_source.source_id, SESSION);
    assert_eq!(row.binding.subject_source.source_revision, 2);
    assert_eq!(
        row.lifecycle.queue_deadline_at,
        reservation.queue_deadline_ms
    );
    match row.payload {
        ReviewPayload::Subject(subject) => {
            assert_eq!(subject, review_subject(&facts(), &aliases()).unwrap())
        }
        other => panic!("{other:?}"),
    }
    // Nothing has published yet.
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());

    let result = rig.publish(Some(&prepared), None, t0() + 2).unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Activated)
    );
    assert_eq!(result.memory_reviewer_nonadmission_count, 0);
    let job = rig.job(&reservation.causal_identity);
    match job.state {
        MemoryReviewerJobState::Ready(input) => {
            assert_eq!(input.subject, job.target);
            assert!(input.starting_references.is_empty());
        }
        other => panic!("{other:?}"),
    }
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(5));
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(
        after
            .meta
            .history_summarizer
            .memory_reviewer_nonadmission
            .count,
        0
    );
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
    assert_eq!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, t0() + 2)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn every_interruption_between_reservation_and_publication_converges_on_one_job() {
    // Window 1: the reservation exists but the durable state was never written.
    let rig = Rig::open();
    let firing = rig.state();
    let first = reserve_and_stage(
        &rig.target(),
        &HandoffRequest {
            store: &rig.store,
            project: PROJECT,
            session_id: SESSION,
            firing: &firing,
            facts: &facts(),
            aliases: &aliases(),
            now_ms: t0(),
        },
        |_| {
            Err(HistorySummarizerStateError::Store(
                memory_store::MemoryStoreError::Serde("crash before persist".into()),
            ))
        },
    );
    assert!(matches!(first, Err(HandoffError::Persist(_))), "{first:?}");
    assert_eq!(rig.state().memory_reviewer_reservation, None);
    let rows_after_window_1 = rig.store.memory_reviewer_headroom(PROJECT).unwrap();
    // Window 2: the state is written but staging never ran (simulated by a fresh call that finds its own reservation).
    let prepared = activation(rig.handoff(t0() + 10).unwrap());
    assert_eq!(
        rig.store.memory_reviewer_headroom(PROJECT).unwrap(),
        rows_after_window_1
    );
    // Windows 3 and 4: staging and sealing are repeated; the same reference and the same job come back.
    let again = activation(rig.handoff(t0() + 20).unwrap());
    assert_eq!(again.causal_identity, prepared.causal_identity);
    assert_eq!(again.input, prepared.input);
    let reservation = rig.reservation();
    assert_eq!(
        rig.read_subject(&reservation, t0() + 21)
            .unwrap()
            .lifecycle
            .queue_deadline_at,
        reservation.queue_deadline_ms
    );
    // Window 5: publication fails after the reservation for a reason a retry can cure (here the activation names the wrong producer); the reservation, the Reserved row, and the staged subject stay, and nothing advances.
    let mut foreign = prepared.clone();
    foreign.producer.firing_id = format!("{SESSION}#99");
    let refused = rig.publish(Some(&foreign), None, t0() + 30);
    assert!(
        matches!(
            refused,
            Err(
                crate::history_summarizer::HistorySummarizerStateError::Publish(
                    HistorySummarizerPublishError::MemoryReviewerActivation(_)
                )
            )
        ),
        "{refused:?}"
    );
    assert_eq!(
        rig.job(&prepared.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    let retained = rig.state();
    assert_eq!(retained.state, HistorySummarizerPhase::Publishing);
    assert_eq!(
        retained.memory_reviewer_reservation,
        Some(reservation.clone())
    );
    assert_eq!(retained.memory_reviewer_nonadmission.count, 0);
    // The retry reconciles against the retained reservation and publishes once.
    let retried = activation(rig.handoff(t0() + 40).unwrap());
    assert_eq!(retried.causal_identity, prepared.causal_identity);
    assert_eq!(
        rig.reservation(),
        reservation,
        "the reservation is reused, not re-reserved"
    );
    let result = rig.publish(Some(&retried), None, t0() + 41).unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Activated)
    );
    assert_eq!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, t0() + 41)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_fence_refusal_after_the_reservation_settles_the_job_without_progress() {
    // The selected input changed under the firing: the history the candidate was extracted from can never publish, so the job is finished as not admitted, the reservation is dropped, nothing advances, and no nonadmission is counted (the outcome lives on the job).
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let mut changed = rig.store.load(SESSION).unwrap();
    changed.meta.block_identity_by_mid.get_mut("m2").unwrap()[0].byte_fingerprint =
        "content-b".to_string();
    rig.store
        .commit(SESSION, changed.row_version, &changed.core, &changed.meta)
        .unwrap();
    let fenced = rig.publish(Some(&prepared), None, t0() + 1);
    assert!(
        matches!(
            fenced,
            Err(
                crate::history_summarizer::HistorySummarizerStateError::Publish(
                    HistorySummarizerPublishError::FenceRejected { .. }
                )
            )
        ),
        "{fenced:?}"
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    let after = rig.store.load(SESSION).unwrap();
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(after.meta.publication_floor_ordinal, None);
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(rig.pending(), None);
    assert_eq!(
        after
            .meta
            .history_summarizer
            .memory_reviewer_nonadmission
            .count,
        0
    );
    assert!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, t0())
            .unwrap()
            .is_empty()
    );
    // A competing publication that moved the segment set on settles the same way.
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    rig.store
        .replace_history_segments(
            SESSION,
            &[memory_store::StoredHistorySegment {
                sequence: 1,
                start_message: 1,
                end_message: 1,
                end_message_id: "m1".into(),
                title: "earlier".into(),
                content: "earlier".into(),
                ..Default::default()
            }],
        )
        .unwrap();
    let fenced = rig.publish(Some(&prepared), None, t0() + 1);
    assert!(matches!(
        fenced,
        Err(
            crate::history_summarizer::HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { .. }
            )
        )
    ));
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    assert_eq!(rig.state().memory_reviewer_reservation, None);
}

#[test]
fn a_late_publication_records_expiry_and_never_resurrects_the_reservation() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let late = t0() + MEMORY_REVIEWER_QUEUE_LIFETIME_MS;
    let result = rig.publish(Some(&prepared), None, late).unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Expired)
    );
    let job = rig.job(&prepared.causal_identity);
    assert_eq!(
        job.state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Expired)
    );
    assert_eq!(
        job.queue_deadline_ms, reservation.queue_deadline_ms,
        "the deadline never moved"
    );
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(
        after.meta.publication_floor_ordinal,
        Some(5),
        "valid history still advances"
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(
        after
            .meta
            .history_summarizer
            .memory_reviewer_nonadmission
            .count,
        0,
        "expiry after reservation is a terminal outcome, not a nonadmission"
    );
    // The staged subject is past its deadline too.
    assert!(matches!(
        rig.read_subject(&reservation, late),
        Err(ReviewReadError::Refused(ReviewReadRefusal::Expired))
    ));
    assert!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, late)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn identical_inputs_neither_duplicate_a_job_nor_reopen_a_settled_one() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    rig.publish(Some(&prepared), None, t0() + 1).unwrap();
    // A later firing with the same accepted facts finds the Ready row and hands off nothing.
    rig.persist(publishing_state(4));
    assert_eq!(rig.handoff(t0() + 2).unwrap(), Handoff::Settled);
    assert_eq!(rig.state().memory_reviewer_reservation, None);
    assert_eq!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, t0() + 2)
            .unwrap()
            .len(),
        1
    );
    // Once the job is terminal, the same inputs stay settled.
    rig.store
        .finish_memory_reviewer_job(
            PROJECT,
            &prepared.causal_identity,
            MemoryReviewerJobOutcome::Failed,
            t0() + 3,
        )
        .unwrap();
    assert_eq!(rig.handoff(t0() + 4).unwrap(), Handoff::Settled);
    // The firing publishes its own history with nothing recorded.
    let mut next = publishing_state(4);
    next.history_segment_set_generation = HistorySegmentSetGeneration {
        max_sequence: 1,
        count: 1,
    };
    rig.persist(next);
    let publication = rig.publish_range(None, None, t0() + 5, 5, 6).unwrap();
    assert_eq!(
        rig.store
            .load(SESSION)
            .unwrap()
            .meta
            .publication_floor_ordinal,
        Some(7)
    );
    assert_eq!(
        publication.memory_reviewer_nonadmission_count, 0,
        "settled inputs are not a nonadmission"
    );
}

#[test]
fn capacity_refusal_before_the_reservation_is_the_recorded_nonadmission() {
    let rig = Rig::open();
    // Fill the project's pending capacity with unrelated reservations.
    for index in 0..MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT {
        rig.store
            .reserve_memory_reviewer_job(
                PROJECT,
                &ProducerBinding {
                    producer: "filler".to_string(),
                    firing_id: format!("fill-{index}"),
                    ordinal: index as u64,
                },
                &CausalInputs {
                    target: ReviewTarget::Memory {
                        object_id: format!("memory-{index}"),
                        source_revision: 1,
                    },
                    question_template: "extracted_facts".to_string(),
                    signals: Vec::new(),
                    required_evidence: Vec::new(),
                    policy_versions: BTreeMap::new(),
                },
                t0(),
            )
            .unwrap();
    }
    let handoff = rig.handoff(t0() + 1).unwrap();
    assert_eq!(
        handoff,
        Handoff::Nonadmission(MemoryReviewerNonadmissionCode::CapacityFull)
    );
    assert_eq!(
        rig.state().memory_reviewer_reservation,
        None,
        "no reservation was persisted"
    );
    // No subject was staged for it.
    let missing = rig.kernel.read_review_input(
        &rig.staged_reference(),
        &review_binding(PROJECT_DIGEST, DOMAIN, SESSION, 2, "job"),
        t0() + 1,
    );
    assert!(
        matches!(
            missing,
            Err(ReviewReadError::Refused(ReviewReadRefusal::Missing))
        ),
        "{missing:?}"
    );
    // The publication records the refusal with its progress.
    let published = rig
        .publish(
            None,
            Some(MemoryReviewerNonadmissionCode::CapacityFull),
            t0() + 2,
        )
        .unwrap();
    assert_eq!(published.memory_reviewer_nonadmission_count, 1);
    let state = rig.state();
    assert_eq!(
        state
            .memory_reviewer_nonadmission
            .latest
            .map(|latest| latest.code),
        Some(MemoryReviewerNonadmissionCode::CapacityFull)
    );
    assert_eq!(
        state
            .memory_reviewer_nonadmission
            .latest
            .map(|latest| latest.firing_seq),
        Some(3)
    );
}

#[test]
fn the_predicate_is_unchanged_by_the_reservation() {
    // The reservation rides in the durable state without becoming part of the publish predicate, so the fences it checks are the ones the firing snapshot fixed.
    let rig = Rig::open();
    let before: HistorySummarizerPublishPredicate =
        crate::history_summarizer::publish_predicate(&rig.state()).unwrap();
    let _ = activation(rig.handoff(t0()).unwrap());
    let after = crate::history_summarizer::publish_predicate(&rig.state()).unwrap();
    assert_eq!(before, after);
}

/// The production publication path: an accepted, cited fact set from the awaiting state is reserved, staged, and activated with its history; a second firing carrying a rejected set records the nonadmission instead and leaves the first job alone.
#[test]
fn the_publication_path_hands_accepted_facts_off_and_records_rejected_ones() {
    use crate::history_summarizer_producer::ProducerOutput;
    use crate::history_summarizer_validate::{
        ChunkLine, HistorySummarizerChunk, StoredHistorySegmentRange, ValidateOptions,
    };
    use memory_store::ExtractionFailure;

    let rig = Rig::open();
    let mut awaiting = publishing_state(3);
    awaiting.state = HistorySummarizerPhase::AwaitingProducer;
    awaiting.history_segment_set_generation = HistorySegmentSetGeneration::default();
    rig.persist(awaiting);
    let chunk = HistorySummarizerChunk {
        aliases: aliases(),
        start_index: 2,
        end_index: 4,
        lines: (2..=4)
            .map(|ordinal| ChunkLine {
                ordinal,
                message_id: format!("m{ordinal}"),
                anchorable: true,
            })
            .collect(),
        present_ordinals: vec![2, 3, 4],
        tool_only_ranges: vec![],
        completed_tool_arcs: vec![],
    };
    let output = |facts: &str| ProducerOutput {
        text: format!(
            r#"<output><history_segments><history_segment start="2" end="3" title="arc" episode_type="feature" importance="60"><p1>arc</p1><p2>arc</p2><p3>arc</p3><p4 /></history_segment></history_segments><facts><PROJECT_RULES>
{facts}
</PROJECT_RULES></facts><meta><unprocessed_from>4</unprocessed_from></meta></output>"#
        ),
        length_capped: false,
    };
    let target = rig.target();
    let publish = |output: ProducerOutput,
                   chunk: &HistorySummarizerChunk,
                   prior: &[StoredHistorySegmentRange]| {
        let awaiting = rig.state();
        publish_output_from_awaiting(PublishOutputRequest {
            store: &rig.store,
            session_id: SESSION,
            project_path: PROJECT,
            awaiting,
            output,
            observed_chunk_fingerprint: "fp",
            validation_chunk: chunk,
            chunk_transcript: "U: transcript",
            boundary_dates: &BTreeMap::new(),
            prior_history_segments: prior,
            validate_options: ValidateOptions {
                in_emergency: true,
                ..ValidateOptions::default()
            },
            created_at_ms: t0(),
            failure_started_at_ms: t0(),
            failure_backoff_at_ms: 0,
            completion_now_ms: t0,
            publication_fence: None,
            memory_reviewer_handoff: Some(&target),
        })
    };

    publish(
        output("* [s1:0-11] [s2:0-22] Run bun install before building."),
        &chunk,
        &[],
    )
    .expect("accepted facts publish with their handoff");
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(4));
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(
        after
            .meta
            .history_summarizer
            .memory_reviewer_nonadmission
            .count,
        0
    );
    let ready = rig
        .store
        .ready_memory_reviewer_jobs(PROJECT, 8, t0())
        .unwrap();
    assert_eq!(ready.len(), 1);
    let job = &ready[0];
    assert_eq!(job.producer.firing_id, format!("{}#3", rig_key()));
    let MemoryReviewerJobState::Ready(input) = &job.state else {
        panic!("{:?}", job.state);
    };
    let ReviewTarget::StagedSubject {
        candidate_id,
        payload_digest,
        ..
    } = &input.subject
    else {
        panic!("{:?}", input.subject);
    };
    let row = rig
        .kernel
        .read_review_input(
            &ReviewStagedReference {
                database_incarnation_id: rig.kernel_incarnation.clone(),
                candidate_id: candidate_id.clone(),
                payload_digest: payload_digest.clone(),
            },
            &review_binding(
                PROJECT_DIGEST,
                DOMAIN,
                SESSION,
                job.producer.ordinal,
                &job.causal_identity,
            ),
            t0() + 1,
        )
        .unwrap();
    let ReviewPayload::Subject(subject) = row.payload else {
        panic!("{:?}", row.payload);
    };
    assert_eq!(subject.facts.len(), 1);
    assert_eq!(subject.facts[0].text, "Run bun install before building.");
    assert_eq!(subject.origins.len(), 2);

    // Firing 4 (4..=6): a rejected set is a recorded nonadmission; the Ready job is untouched and no reservation is made.
    let mut next = publishing_state(4);
    next.state = HistorySummarizerPhase::AwaitingProducer;
    next.chunk_range = Some(HistorySummarizerChunkRange {
        from_ordinal: 4,
        to_ordinal: 6,
    });
    next.history_segment_set_generation = HistorySegmentSetGeneration {
        max_sequence: 1,
        count: 1,
    };
    next.memory_reviewer_nonadmission = after.meta.history_summarizer.memory_reviewer_nonadmission;
    rig.persist(next);
    let later = HistorySummarizerChunk {
        aliases: FrozenAliasTable::default(),
        start_index: 4,
        end_index: 6,
        lines: (4..=6)
            .map(|ordinal| ChunkLine {
                ordinal,
                message_id: format!("m{ordinal}"),
                anchorable: true,
            })
            .collect(),
        present_ordinals: vec![4, 5, 6],
        tool_only_ranges: vec![],
        completed_tool_arcs: vec![],
    };
    let rejected = output("* [s9:0-4] unknown alias")
        .text
        .replace(r#"start="2" end="3""#, r#"start="4" end="5""#)
        .replace("<unprocessed_from>4<", "<unprocessed_from>6<");
    publish(
        ProducerOutput {
            text: rejected,
            length_capped: false,
        },
        &later,
        &[StoredHistorySegmentRange {
            start_message: 2,
            end_message: 3,
        }],
    )
    .expect("history publishes beside the rejected set");
    let state = rig.state();
    assert_eq!(state.memory_reviewer_nonadmission.count, 1);
    assert_eq!(
        state
            .memory_reviewer_nonadmission
            .latest
            .map(|latest| latest.code),
        Some(MemoryReviewerNonadmissionCode::FactSetRejected {
            failure: ExtractionFailure::UnknownAlias,
        })
    );
    assert_eq!(state.memory_reviewer_reservation, None);
    assert_eq!(
        rig.store
            .ready_memory_reviewer_jobs(PROJECT, 8, t0())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn an_activation_that_cannot_apply_rolls_the_whole_publication_back() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    // The activation names a producer the reservation does not belong to.
    let mut foreign = prepared.clone();
    foreign.producer.firing_id = format!("{SESSION}#99");
    let refused = rig.publish(Some(&foreign), None, t0() + 1);
    assert!(
        matches!(
            refused,
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::MemoryReviewerActivation(
                    memory_store::memory_reviewer_jobs::MemoryReviewerJobRefusal::ProducerMismatch
                )
            ))
        ),
        "{refused:?}"
    );
    // Nothing moved on either store: no history row, no floor, the job still Reserved, the reservation still recorded, the subject still staged.
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    let state = rig.store.load(SESSION).unwrap();
    assert_eq!(state.meta.publication_floor_ordinal, None);
    assert_eq!(
        state.meta.history_summarizer.state,
        HistorySummarizerPhase::Publishing
    );
    assert_eq!(
        state.meta.history_summarizer.memory_reviewer_reservation,
        Some(reservation.clone())
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );
    assert!(rig.read_subject(&reservation, t0() + 2).is_ok());

    // A publication that names no activation while a reservation is recorded is refused the same way; so is one whose activation names another job.
    let refused = rig.publish(None, None, t0() + 3);
    assert!(
        matches!(
            refused,
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::MemoryReviewerActivation(
                    memory_store::memory_reviewer_jobs::MemoryReviewerJobRefusal::InvalidRequest
                )
            ))
        ),
        "{refused:?}"
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );

    // The matching activation then publishes once.
    let result = rig.publish(Some(&prepared), None, t0() + 4).unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Activated)
    );
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
}

#[test]
fn a_reservation_the_sweep_already_expired_reads_as_expired_at_late_publication() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let late = t0() + MEMORY_REVIEWER_QUEUE_LIFETIME_MS + 1;
    let (jobs, _) = rig.store.expire_memory_reviewer_work(late).unwrap();
    assert_eq!(jobs, 1);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Expired)
    );
    let result = rig.publish(Some(&prepared), None, late).unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Expired)
    );
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(5));
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).queue_deadline_ms,
        reservation.queue_deadline_ms
    );
}

#[test]
fn restart_republishes_a_reserved_firing_without_a_model_run() {
    let mut rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    // The daemon restarts before publication: the state is Publishing with the reservation and the retained output.
    rig.reopen();
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, t0() + 60_000).unwrap(),
        RestartAction::RepublishReserved { firing_seq: 3 }
    );
    assert_eq!(rig.state().state, HistorySummarizerPhase::Publishing);
    // Without the Kernel the staged subject cannot be verified; nothing moves.
    assert_eq!(rig.republish(None, t0() + 1), RepublishOutcome::Retained);
    assert_eq!(
        rig.state().memory_reviewer_reservation,
        Some(reservation.clone())
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    // With it, the retained publication commits and activates the same job.
    let target = rig.target();
    let outcome = rig.republish(Some(&target), t0() + 2);
    assert_eq!(outcome, RepublishOutcome::Published);
    let job = rig.job(&reservation.causal_identity);
    assert!(
        matches!(job.state, MemoryReviewerJobState::Ready(_)),
        "{:?}",
        job.state
    );
    assert_eq!(job.causal_identity, prepared.causal_identity);
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(5));
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(rig.pending(), None);
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
    // A second restart finds nothing to do.
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, t0() + 60_000).unwrap(),
        RestartAction::Done
    );
}

#[test]
fn restart_settles_a_reserved_firing_whose_input_changed_or_expired() {
    // Changed selected input: the republication is fenced, the job is finished as not admitted, and the run is idle with nothing advanced.
    let mut rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let mut changed = rig.store.load(SESSION).unwrap();
    changed.meta.block_identity_by_mid.get_mut("m2").unwrap()[0].byte_fingerprint =
        "content-b".to_string();
    rig.store
        .commit(SESSION, changed.row_version, &changed.core, &changed.meta)
        .unwrap();
    rig.reopen();
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, t0() + 60_000).unwrap(),
        RestartAction::RepublishReserved { firing_seq: 3 }
    );
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 1),
        RepublishOutcome::Settled
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(
        rig.store
            .load(SESSION)
            .unwrap()
            .meta
            .publication_floor_ordinal,
        None
    );

    // Expired reservation: the late republication records expiry and the valid history still advances, once.
    let mut rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    rig.reopen();
    let late = t0() + MEMORY_REVIEWER_QUEUE_LIFETIME_MS + 1;
    let target = rig.target();
    let outcome = rig.republish(Some(&target), late);
    assert_eq!(outcome, RepublishOutcome::Published);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Expired)
    );
    assert_eq!(
        rig.store
            .load(SESSION)
            .unwrap()
            .meta
            .publication_floor_ordinal,
        Some(5)
    );
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, late).unwrap(),
        RestartAction::Done
    );
}

#[test]
fn a_production_reservation_republishes_after_a_restart_and_a_stale_one_is_not_carried() {
    use crate::history_summarizer_producer::ProducerOutput;
    use crate::history_summarizer_validate::{ChunkLine, HistorySummarizerChunk, ValidateOptions};

    // The production path reserves and retains, then the store fails to activate (the activation names the wrong producer), which keeps the firing in Publishing with the failure recorded.
    let mut rig = Rig::open();
    let mut awaiting = publishing_state(3);
    awaiting.state = HistorySummarizerPhase::AwaitingProducer;
    awaiting.history_segment_set_generation = HistorySegmentSetGeneration::default();
    rig.persist(awaiting);
    let chunk = HistorySummarizerChunk {
        aliases: aliases(),
        start_index: 2,
        end_index: 4,
        lines: (2..=4)
            .map(|ordinal| ChunkLine {
                ordinal,
                message_id: format!("m{ordinal}"),
                anchorable: true,
            })
            .collect(),
        present_ordinals: vec![2, 3, 4],
        tool_only_ranges: vec![],
        completed_tool_arcs: vec![],
    };
    let target = rig.target();
    // A fence that refuses once, as a retired transform snapshot does, and then admits.
    struct RefuseOnce(std::sync::atomic::AtomicBool);
    impl HistorySummarizerPublicationFence for RefuseOnce {
        fn publish(
            &self,
            store: &MemoryStore,
            request: memory_store::HistorySummarizerPublishRequest<'_>,
        ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerPublishError>
        {
            if self.0.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return Err(HistorySummarizerPublishError::CallerFenceRejected {
                    reason: "snapshot retired".to_string(),
                });
            }
            store.publish_history_summarizer_chunk(request)
        }
    }
    let fence = RefuseOnce(std::sync::atomic::AtomicBool::new(true));
    let refused = publish_output_from_awaiting(PublishOutputRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        awaiting: rig.state(),
        output: ProducerOutput {
            text: r#"<output><history_segments><history_segment start="2" end="3" title="arc" episode_type="feature" importance="60"><p1>arc</p1><p2>arc</p2><p3>arc</p3><p4 /></history_segment></history_segments><facts><PROJECT_RULES>
* [s1:0-11] [s2:0-22] Run bun install before building.
</PROJECT_RULES></facts><meta><unprocessed_from>4</unprocessed_from></meta></output>"#
                .to_string(),
            length_capped: false,
        },
        observed_chunk_fingerprint: "fp",
        validation_chunk: &chunk,
        chunk_transcript: "U: the transcript the recovery republishes",
        boundary_dates: &BTreeMap::new(),
        prior_history_segments: &[],
        validate_options: ValidateOptions {
            in_emergency: true,
            ..ValidateOptions::default()
        },
        created_at_ms: t0(),
        failure_started_at_ms: t0(),
        failure_backoff_at_ms: 0,
        completion_now_ms: t0,
        publication_fence: Some(&fence),
        memory_reviewer_handoff: Some(&target),
    });
    assert!(
        matches!(
            refused,
            Err(HistorySummarizerDriveError::State(
                HistorySummarizerStateError::Publish(
                    HistorySummarizerPublishError::CallerFenceRejected { .. }
                )
            ))
        ),
        "{refused:?}"
    );
    let retained = rig.state();
    assert_eq!(retained.state, HistorySummarizerPhase::Publishing);
    let reservation = retained
        .memory_reviewer_reservation
        .clone()
        .expect("retained");
    assert_eq!(retained.consecutive_publish_failures, 1);
    let (firing_seq, pending) = rig.pending().expect("the retained publication is stored");
    assert_eq!(firing_seq, 3);
    assert_eq!(
        pending.chunk_transcript,
        "U: the transcript the recovery republishes"
    );
    assert_eq!(pending.publication_floor_ordinal, 4);
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );

    // After a restart, recovery republishes the retained output: no model, one activation, history advanced to exactly what the firing validated.
    rig.reopen();
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, t0() + 60_000).unwrap(),
        RestartAction::RepublishReserved { firing_seq: 3 }
    );
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 1),
        RepublishOutcome::Published
    );
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(4));
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(
        after.meta.history_summarizer.memory_reviewer_reservation,
        None
    );
    assert_eq!(rig.pending(), None);
    let segments = rig.store.load_history_segments(SESSION).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!((segments[0].start_message, segments[0].end_message), (2, 3));
    assert!(matches!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Ready(_)
    ));
    assert_eq!(
        rig.store
            .load_chunk_transcripts_for_range(SESSION, 2, 3)
            .unwrap()
            .len(),
        1,
        "the retained transcript published with the segment"
    );

    // A reservation left by an earlier firing is not carried into the next one.
    let mut stale = after.meta.history_summarizer.clone();
    stale.memory_reviewer_reservation = Some(reservation.clone());
    rig.persist(stale);
    let fired = match fire(
        &rig.state(),
        4,
        6,
        "fp".into(),
        selected_range_identities(),
        0,
        HistorySegmentSetGeneration {
            max_sequence: 1,
            count: 1,
        },
        t0() + 2,
    )
    .unwrap()
    {
        FireOutcome::Fired(state) => state,
        FireOutcome::Busy(_) => unreachable!(),
    };
    assert_eq!(fired.memory_reviewer_reservation, None);
    assert_eq!(fired.firing_seq, 4);
}

#[test]
fn a_row_conflict_retains_the_reservation_and_a_duplicate_settle_spares_a_ready_job() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    // Another writer commits the session row before the publication: the CAS fails, the reservation and the retained output stay, and the state remains Publishing for the next pass.
    let loaded = rig.store.load(SESSION).unwrap();
    let predicate = publish_predicate(&loaded.meta.history_summarizer).unwrap();
    rig.store
        .commit(SESSION, loaded.row_version, &loaded.core, &loaded.meta)
        .unwrap();
    let conflict = rig.publish_at(
        loaded.row_version,
        &predicate,
        Some(&prepared),
        None,
        t0() + 1,
        2,
        4,
    );
    assert!(
        matches!(
            conflict,
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::CasConflict { reason: None, .. }
            ))
        ),
        "{conflict:?}"
    );
    let state = rig.state();
    assert_eq!(state.state, HistorySummarizerPhase::Publishing);
    assert_eq!(state.memory_reviewer_reservation, Some(reservation.clone()));
    assert!(rig.pending().is_some());
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    // Recovery publishes it.
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 2),
        RepublishOutcome::Published
    );
    assert!(matches!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Ready(_)
    ));
    // A late duplicate publication of the same activation is refused by the row version and does not settle the Ready job.
    let late = rig.publish_at(
        loaded.row_version,
        &predicate,
        Some(&prepared),
        None,
        t0() + 3,
        2,
        4,
    );
    assert!(matches!(
        late,
        Err(HistorySummarizerStateError::Publish(
            HistorySummarizerPublishError::CasConflict { .. }
                | HistorySummarizerPublishError::InvalidState { .. }
        ))
    ));
    assert!(matches!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Ready(_)
    ));
}

#[test]
fn an_unreadable_retained_publication_settles_the_reservation_instead_of_stranding_the_firing() {
    // The retained payload is the daemon's own serialization of in-memory types; a later daemon that no longer reads it must still close the firing, or the session never leaves Publishing.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    rig.retain(
        &reservation,
        &memory_store::PendingPublication {
            validated_json: r#"{"schema":"a shape this daemon does not read"}"#.to_string(),
            ..pending_publication(&validated_range(2, 4))
        },
    );
    let target = rig.target();
    let outcome = republish_reserved(RepublishRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        memory_reviewer_handoff: Some(&target),
        now_ms: t0() + 1,
        failure_backoff_at_ms: t0() + 60_000,
        publication_fence: None,
        collect_user_memory_candidates: false,
        memory_enabled: true,
    });
    assert!(
        matches!(outcome, Ok(RepublishOutcome::Settled)),
        "{outcome:?}"
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, t0() + 60_000).unwrap(),
        RestartAction::Done
    );
}

#[test]
fn a_reincarnated_kernel_settles_the_reservation_without_reserving_a_second_job() {
    // The reservation names its subject by digest and Kernel incarnation. A Kernel that came back under a new incarnation no longer holds that subject, and a handoff under the new incarnation would reserve a second job the publication then refuses; recovery must settle before it reserves or stages anything.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let reincarnated = HandoffTarget {
        kernel_incarnation: "f".repeat(64),
        ..rig.target()
    };
    assert_eq!(
        rig.republish(Some(&reincarnated), t0() + 1),
        RepublishOutcome::Settled
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    assert_eq!(
        rig.store
            .memory_reviewer_headroom(PROJECT)
            .unwrap()
            .pending_jobs,
        0,
        "no job was reserved under the new incarnation"
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
}

#[test]
fn every_retained_republish_arms_the_backoff() {
    // Recovery runs on every transform pass while the firing is not idle, so a pass that decides nothing must leave a backoff for the next one to wait on.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    assert_eq!(rig.state().failure_backoff_at_ms, None);
    // No handoff target: nothing can be verified.
    assert_eq!(rig.republish(None, t0() + 1), RepublishOutcome::Retained);
    let retained = rig.state();
    assert_eq!(retained.state, HistorySummarizerPhase::Publishing);
    assert_eq!(
        retained.memory_reviewer_reservation,
        Some(reservation.clone())
    );
    assert_eq!(retained.failure_backoff_at_ms, Some(t0() + 1 + 60_000));
    // Another writer moved the session row under the publication.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    struct MoveRowFirst<'a>(&'a MemoryStore);
    impl HistorySummarizerPublicationFence for MoveRowFirst<'_> {
        fn publish(
            &self,
            store: &MemoryStore,
            request: memory_store::HistorySummarizerPublishRequest<'_>,
        ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerPublishError>
        {
            let loaded = self.0.load(SESSION).unwrap();
            self.0
                .commit(SESSION, loaded.row_version, &loaded.core, &loaded.meta)
                .unwrap();
            store.publish_history_summarizer_chunk(request)
        }
    }
    let target = rig.target();
    let fence = MoveRowFirst(&rig.store);
    let outcome = republish_reserved(RepublishRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        memory_reviewer_handoff: Some(&target),
        now_ms: t0() + 1,
        failure_backoff_at_ms: t0() + 1 + 60_000,
        publication_fence: Some(&fence),
        collect_user_memory_candidates: false,
        memory_enabled: true,
    })
    .unwrap();
    assert_eq!(outcome, RepublishOutcome::Retained);
    let retained = rig.state();
    assert_eq!(retained.state, HistorySummarizerPhase::Publishing);
    assert_eq!(retained.failure_backoff_at_ms, Some(t0() + 1 + 60_000));
    assert_eq!(
        rig.republish(Some(&target), t0() + 2),
        RepublishOutcome::Published
    );
}

#[test]
fn a_settle_from_a_stale_snapshot_spares_a_later_firing_and_the_job_the_publication_activated() {
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    // The publication commits and activates the job, but its answer is lost: by the time a refusal reaches this pass, firing 4 has fired and retained its own publication.
    struct CommitThenMoveOn<'a>(&'a Rig);
    impl HistorySummarizerPublicationFence for CommitThenMoveOn<'_> {
        fn publish(
            &self,
            store: &MemoryStore,
            request: memory_store::HistorySummarizerPublishRequest<'_>,
        ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerPublishError>
        {
            store.publish_history_summarizer_chunk(request)?;
            let rig = self.0;
            rig.persist(next_publishing_firing(rig, 5, 6));
            rig.retain(
                &later_reservation(),
                &pending_publication(&validated_range(5, 6)),
            );
            Err(HistorySummarizerPublishError::MemoryReviewerActivation(
                MemoryReviewerJobRefusal::InvalidRequest,
            ))
        }
    }
    let target = rig.target();
    let fence = CommitThenMoveOn(&rig);
    let outcome = republish_reserved(RepublishRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        memory_reviewer_handoff: Some(&target),
        now_ms: t0() + 1,
        failure_backoff_at_ms: t0() + 1 + 60_000,
        publication_fence: Some(&fence),
        collect_user_memory_candidates: false,
        memory_enabled: true,
    })
    .unwrap();
    // The activated job, the later firing, and its retained publication are all someone else's now.
    assert!(
        matches!(
            rig.job(&reservation.causal_identity).state,
            MemoryReviewerJobState::Ready(_)
        ),
        "{:?}",
        rig.job(&reservation.causal_identity).state
    );
    let later = rig.state();
    assert_eq!(later.state, HistorySummarizerPhase::Publishing);
    assert_eq!(later.firing_seq, 4);
    assert_eq!(later.memory_reviewer_reservation, Some(later_reservation()));
    assert_eq!(rig.pending().map(|(firing_seq, _)| firing_seq), Some(4));
    assert_eq!(outcome, RepublishOutcome::Retained);
}

/// A reservation firing 4 records for a job of its own.
fn later_reservation() -> memory_store::MemoryReviewerReservation {
    memory_store::MemoryReviewerReservation {
        firing_seq: 4,
        causal_identity: "later-job".to_string(),
        candidate_id: "hs-ses-later".to_string(),
        payload_digest: "later".to_string(),
        kernel_incarnation: "kernel".to_string(),
        queue_deadline_ms: t0() + MEMORY_REVIEWER_QUEUE_LIFETIME_MS,
    }
}

#[test]
fn a_retained_publication_the_store_refuses_is_a_nonadmission_before_any_reservation() {
    let rig = Rig::open();
    let target = rig.target();
    let decide = |pending: PendingPublication| {
        let loaded = rig.store.load(SESSION).unwrap();
        memory_reviewer_decision_before_publish(MemoryReviewerDecisionRequest {
            store: &rig.store,
            session_id: SESSION,
            project_path: PROJECT,
            publishing: &loaded.meta.history_summarizer,
            publishing_row_version: loaded.row_version.unwrap(),
            validated: &accepted_range(2, 4),
            aliases: &aliases(),
            pending,
            memory_reviewer_handoff: Some(&target),
            failure_started_at_ms: t0(),
            failure_backoff_at_ms: t0() + 60_000,
            completion_now_ms: t0,
        })
    };
    // A chunk transcript inside its own envelope, retained beside the alias table that presents the same text again, exceeds the transcript envelope alone but fits the publication's.
    let mut pending = pending_publication(&validated_range(2, 4));
    pending.chunk_transcript = incompressible_text(400 * 1024, 1);
    pending.aliases_json = serde_json::to_string(&incompressible_text(400 * 1024, 2)).unwrap();
    let decision = decide(pending).unwrap();
    assert!(decision.memory_reviewer_activation.is_some());
    assert_eq!(rig.pending().map(|(firing_seq, _)| firing_seq), Some(3));
    // Past the envelope, the store refuses the payload before any job is reserved: the publication records a nonadmission, nothing is reserved, and the firing is not abandoned.
    let rig = Rig::open();
    let target = rig.target();
    let mut pending = pending_publication(&validated_range(2, 4));
    pending.chunk_transcript = incompressible_text(500 * 1024, 3);
    pending.aliases_json = serde_json::to_string(&incompressible_text(500 * 1024, 4)).unwrap();
    pending.validated_json = serde_json::to_string(&incompressible_text(500 * 1024, 5)).unwrap();
    let headroom_before = rig.store.memory_reviewer_headroom(PROJECT).unwrap();
    let loaded = rig.store.load(SESSION).unwrap();
    let decision = memory_reviewer_decision_before_publish(MemoryReviewerDecisionRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        publishing: &loaded.meta.history_summarizer,
        publishing_row_version: loaded.row_version.unwrap(),
        validated: &accepted_range(2, 4),
        aliases: &aliases(),
        pending,
        memory_reviewer_handoff: Some(&target),
        failure_started_at_ms: t0(),
        failure_backoff_at_ms: t0() + 60_000,
        completion_now_ms: t0,
    })
    .unwrap();
    assert_eq!(
        decision.memory_reviewer_nonadmission,
        Some(MemoryReviewerNonadmissionCode::SubjectRefused)
    );
    assert!(decision.memory_reviewer_activation.is_none());
    assert_eq!(
        rig.store
            .memory_reviewer_headroom(PROJECT)
            .unwrap()
            .pending_jobs,
        headroom_before.pending_jobs
    );
    assert_eq!(rig.state().state, HistorySummarizerPhase::Publishing);
    assert_eq!(rig.pending(), None);
}

/// The validated chunk with its facts accepted, as the publication path sees an accepted set.
fn accepted_range(start: u64, end: u64) -> ValidatedChunk {
    let mut validated = validated_range(start, end);
    validated.extraction = ExtractionOutcome::Accepted {
        count: validated.facts.len(),
    };
    validated
}

/// `len` bytes of text deflate cannot shrink much: a xorshift stream mapped onto 64 symbols.
fn incompressible_text(len: usize, seed: u64) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ALPHABET[(state >> 58) as usize] as char
        })
        .collect()
}

#[test]
fn a_retain_that_loses_the_row_to_a_publication_writes_nothing_back() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    // The retain checks a Publishing state that holds the reservation; before it writes, another pass publishes the firing, activates the job, and drops the retained publication.
    let outcome = retain_republish(&rig.store, SESSION, 3, |current| {
        rig.publish(Some(&prepared), None, t0() + 1).unwrap();
        retain_with_detail(current, t0() + 60_000, Some("stale pass".to_string()))
    })
    .unwrap();
    assert_eq!(outcome, RepublishOutcome::Retained);
    let state = rig.state();
    assert_eq!(state.state, HistorySummarizerPhase::Idle);
    assert_eq!(state.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
    // Nothing was resurrected, so the next pass finds nothing to settle and the activated job stands.
    assert_eq!(
        handle_restart_load(&rig.store, SESSION, t0() + 2).unwrap(),
        RestartAction::Done
    );
    assert!(matches!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Ready(_)
    ));
}

#[test]
fn a_republished_publication_keeps_the_time_it_was_first_attempted() {
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let target = rig.target();
    // Recovery runs a minute later; the history it publishes was produced at the original attempt, and search ranks recency by that time.
    assert_eq!(
        rig.republish(Some(&target), t0() + 60_000),
        RepublishOutcome::Published
    );
    let segments = rig.store.load_history_segments(SESSION).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].created_at, t0());
}

#[test]
fn a_republication_honors_the_current_user_memory_gate() {
    let rig = Rig::open();
    // The firing was retained while collection was enabled and produced an observation; the setting is off by the time recovery runs.
    let mut validated = validated_range(2, 4);
    validated.user_observations = vec![UserObservationCandidate {
        content: "prefers short answers".to_string(),
        origin_history_segment_index: Some(0),
    }];
    let mut pending = pending_publication(&validated);
    pending.collect_user_memory_candidates = true;
    let handoff = reserve_and_stage(
        &rig.target(),
        &HandoffRequest {
            store: &rig.store,
            project: PROJECT,
            session_id: SESSION,
            firing: &rig.state(),
            facts: &facts(),
            aliases: &aliases(),
            now_ms: t0(),
        },
        |reservation| Ok(rig.retain(reservation, &pending)),
    )
    .unwrap();
    let _ = activation(handoff);
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 1),
        RepublishOutcome::Published
    );
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
    assert!(
        rig.store
            .load_user_memory_candidates(SESSION)
            .unwrap()
            .is_empty(),
        "collection is off now, so the retained observation is not written"
    );
}

#[test]
fn a_retained_publication_the_scanner_would_rewrite_is_a_nonadmission_before_any_reservation() {
    // The alias table presents a secret outside every cited range: the subject digests clean, but the store would retain the table with the secret substituted, and recovery would rebuild a different subject from it.
    let rig = Rig::open();
    let target = rig.target();
    let mut pending = pending_publication(&validated_range(2, 4));
    pending.aliases_json = serde_json::to_string(&serde_json::json!({
        "aliases": [{"presented": "the user typed password=hunter-two and moved on"}]
    }))
    .unwrap();
    let headroom_before = rig.store.memory_reviewer_headroom(PROJECT).unwrap();
    let loaded = rig.store.load(SESSION).unwrap();
    let decision = memory_reviewer_decision_before_publish(MemoryReviewerDecisionRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        publishing: &loaded.meta.history_summarizer,
        publishing_row_version: loaded.row_version.unwrap(),
        validated: &accepted_range(2, 4),
        aliases: &aliases(),
        pending,
        memory_reviewer_handoff: Some(&target),
        failure_started_at_ms: t0(),
        failure_backoff_at_ms: t0() + 60_000,
        completion_now_ms: t0,
    })
    .unwrap();
    assert_eq!(
        decision.memory_reviewer_nonadmission,
        Some(MemoryReviewerNonadmissionCode::SubjectRefused)
    );
    assert!(decision.memory_reviewer_activation.is_none());
    assert_eq!(
        rig.store
            .memory_reviewer_headroom(PROJECT)
            .unwrap()
            .pending_jobs,
        headroom_before.pending_jobs
    );
    assert_eq!(rig.pending(), None);
}

#[test]
fn a_reservation_made_under_an_earlier_memories_authority_settles_instead_of_migrating() {
    // The route was rebound between the reservation and recovery: the job was reserved under "git:other", recovery runs under PROJECT. Recovery republishes only under the job the reservation names; a job it cannot find under the current authority is not replaced, the reservation settles, and the firing refires under the current authority.
    let rig = Rig::open();
    let handoff = reserve_and_stage(
        &rig.target(),
        &HandoffRequest {
            store: &rig.store,
            project: "git:other",
            session_id: SESSION,
            firing: &rig.state(),
            facts: &facts(),
            aliases: &aliases(),
            now_ms: t0(),
        },
        |reservation| Ok(rig.retain(reservation, &pending_publication(&validated_range(2, 4)))),
    )
    .unwrap();
    let earlier = activation(handoff);
    let headroom_before = rig.store.memory_reviewer_headroom(PROJECT).unwrap();
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 1),
        RepublishOutcome::Settled
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(
        rig.store
            .memory_reviewer_headroom(PROJECT)
            .unwrap()
            .pending_jobs,
        headroom_before.pending_jobs,
        "no replacement job is reserved under the current authority"
    );
    // The earlier project's job is left to its expiry sweep.
    assert_eq!(
        rig.store
            .lookup_memory_reviewer_job("git:other", &earlier.causal_identity)
            .unwrap()
            .unwrap()
            .state,
        MemoryReviewerJobState::Reserved
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
}

#[test]
fn a_reservation_whose_job_is_no_longer_reserved_for_its_firing_settles() {
    // The job the reservation names is already terminal under the current project (another firing's publication under the same identity closed it). Recovery cannot activate it and does not wait for the deadline: the reservation settles and the firing refires.
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    rig.store
        .finish_memory_reviewer_job(
            PROJECT,
            &prepared.causal_identity,
            MemoryReviewerJobOutcome::Nonadmitted,
            t0() + 1,
        )
        .unwrap();
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 2),
        RepublishOutcome::Settled
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
}

#[test]
fn a_fence_refusal_still_abandons_the_firing_when_its_job_is_not_under_the_current_project() {
    // The job was reserved under "git:other"; the fenced publication under PROJECT is refused because the selected input changed. The reservation is dropped, the job cannot be finished from here, and the firing must still return to Idle with the refusal reported.
    let rig = Rig::open();
    let handoff = reserve_and_stage(
        &rig.target(),
        &HandoffRequest {
            store: &rig.store,
            project: "git:other",
            session_id: SESSION,
            firing: &rig.state(),
            facts: &facts(),
            aliases: &aliases(),
            now_ms: t0(),
        },
        |reservation| Ok(rig.retain(reservation, &pending_publication(&validated_range(2, 4)))),
    )
    .unwrap();
    let prepared = activation(handoff);
    let mut changed = rig.store.load(SESSION).unwrap();
    changed.meta.block_identity_by_mid.get_mut("m2").unwrap()[0].byte_fingerprint =
        "content-b".to_string();
    rig.store
        .commit(SESSION, changed.row_version, &changed.core, &changed.meta)
        .unwrap();
    let refused = rig.publish(Some(&prepared), None, t0() + 1);
    assert!(
        matches!(
            refused,
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { .. }
            ))
        ),
        "{refused:?}"
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
}

/// Equal fact sets encode to equal bytes whatever order the model emitted them in, so a reordered retry names the same candidate and adopts the same reservation.
#[test]
fn the_subject_is_the_same_whatever_order_the_facts_arrive_in() {
    let mut reversed = facts();
    reversed.reverse();
    assert_eq!(
        review_subject(&facts(), &aliases()).unwrap(),
        review_subject(&reversed, &aliases()).unwrap()
    );
}

/// The wire protocol admits a 256-byte session id; every identity the handoff derives from it stays inside the store's identity bound.
#[test]
fn a_session_id_at_the_wire_bound_still_hands_off() {
    let rig = Rig::open();
    let session_id = "s".repeat(256);
    let firing = publishing_state(3);
    let handoff = reserve_and_stage(
        &rig.target(),
        &HandoffRequest {
            store: &rig.store,
            project: PROJECT,
            session_id: &session_id,
            firing: &firing,
            facts: &facts(),
            aliases: &aliases(),
            now_ms: t0(),
        },
        |_| Ok(1),
    )
    .unwrap();
    assert!(matches!(handoff, Handoff::Activate(_)), "{handoff:?}");
}

/// A recorded reservation names this firing's job only when it is the job this firing would reserve now. One made under other policy versions, as before a process upgrade, is left to expire and the new policy gets its own job (Q25/Q29).
#[test]
fn a_reservation_under_other_policy_versions_is_not_adopted() {
    use crate::memory_reviewer::broker::QuestionTemplate;

    let rig = Rig::open();
    let reference = rig.staged_reference();
    let inputs = |policy_versions: BTreeMap<String, String>| CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: rig.kernel_incarnation.clone(),
            candidate_id: reference.candidate_id.clone(),
            payload_digest: reference.payload_digest.clone(),
        },
        question_template: QuestionTemplate::ExtractedFacts.id().to_string(),
        signals: Vec::new(),
        required_evidence: Vec::new(),
        policy_versions,
    };
    let current = inputs(review_policy_versions()).causal_identity().unwrap();
    // A prior firing reserved the same subject under an older step schema and never published.
    let prior = ProducerBinding {
        producer: PRODUCER.to_string(),
        firing_id: "prior-firing".to_string(),
        ordinal: 2,
    };
    let previous_policy = inputs(BTreeMap::from([(
        "step_schema".to_string(),
        "previous".to_string(),
    )]));
    let previous = match rig
        .store
        .reserve_memory_reviewer_job(PROJECT, &prior, &previous_policy, t0())
        .unwrap()
    {
        memory_store::memory_reviewer_jobs::ReserveOutcome::Reserved(job) => job,
        other => panic!("{other:?}"),
    };
    assert_ne!(previous.causal_identity, current);
    let mut state = publishing_state(3);
    state.memory_reviewer_reservation = Some(memory_store::MemoryReviewerReservation {
        firing_seq: 2,
        causal_identity: previous.causal_identity.clone(),
        candidate_id: reference.candidate_id,
        payload_digest: reference.payload_digest,
        kernel_incarnation: rig.kernel_incarnation.clone(),
        queue_deadline_ms: previous.queue_deadline_ms,
    });
    rig.persist(state);
    let adopted = activation(rig.handoff(t0() + 1).unwrap());
    assert_eq!(
        adopted.causal_identity, current,
        "the firing reserves under the policy it runs, not the one it recorded"
    );
    assert_eq!(
        rig.job(&previous.causal_identity).state,
        MemoryReviewerJobState::Reserved,
        "the previous policy's reservation is left for the sweep"
    );
}

/// A handoff failure abandons the firing that reserved, fenced on its publish predicate; a newer firing that moved the session on is untouched.
#[test]
fn a_handoff_failure_abandons_only_the_firing_that_reserved() {
    let rig = Rig::open();
    let publishing = rig.state();
    let publishing_row_version = rig.store.load(SESSION).unwrap().row_version.unwrap();
    // Another writer moved the session on to firing 4 before the reservation was recorded.
    let mut next = publishing_state(4);
    next.state = HistorySummarizerPhase::AwaitingProducer;
    rig.persist(next.clone());
    let mut validated = validated_range(2, 4);
    validated.extraction = ExtractionOutcome::Accepted { count: 2 };
    let target = rig.target();
    let result = memory_reviewer_decision_before_publish(MemoryReviewerDecisionRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        publishing: &publishing,
        publishing_row_version,
        validated: &validated,
        aliases: &aliases(),
        pending: pending_publication(&validated),
        memory_reviewer_handoff: Some(&target),
        failure_started_at_ms: t0(),
        failure_backoff_at_ms: 0,
        completion_now_ms: t0,
    });
    assert!(
        matches!(
            result,
            Err(HistorySummarizerDriveError::MemoryReviewerHandoff(
                HandoffError::Persist(HistorySummarizerStateError::Publish(
                    HistorySummarizerPublishError::CasConflict { .. }
                ))
            ))
        ),
        "{:?}",
        result.err()
    );
    assert_eq!(
        rig.state(),
        next,
        "the newer firing is not abandoned by the one that lost the race"
    );
}

/// V27: one session id routed under two project roots is two sessions. Equal facts from both stage as distinct Kernel rows instead of the second colliding with the first's witness.
#[test]
fn the_same_session_under_two_project_roots_stages_two_subjects() {
    let rig = Rig::open();
    let firing = publishing_state(3);
    let mut other_root = rig.target();
    other_root.project_digest = "b".repeat(64);
    for (project, target) in [(PROJECT, rig.target()), ("git:other", other_root)] {
        let handoff = reserve_and_stage(
            &target,
            &HandoffRequest {
                store: &rig.store,
                project,
                session_id: SESSION,
                firing: &firing,
                facts: &facts(),
                aliases: &aliases(),
                now_ms: t0(),
            },
            |_| Ok(1),
        )
        .unwrap();
        assert!(matches!(handoff, Handoff::Activate(_)), "{handoff:?}");
    }
}

/// Q25/Q29: a policy change permits one new job at an unchanged subject, and the Kernel row it stages must be its own, so the policies are part of the row's identity.
#[test]
fn the_handoff_key_separates_projects_sessions_and_policies() {
    let current = review_policy_versions();
    let mut previous = current.clone();
    previous.insert("step_schema".to_string(), "previous".to_string());
    let key = handoff_key(PROJECT_DIGEST, SESSION, &current);
    assert_eq!(key.len(), 32);
    assert_ne!(key, handoff_key(&"b".repeat(64), SESSION, &current));
    assert_ne!(key, handoff_key(PROJECT_DIGEST, "other", &current));
    assert_ne!(key, handoff_key(PROJECT_DIGEST, SESSION, &previous));
}

/// The producer wait can reach ten minutes; the queue lifetime starts when capacity is reserved, not when the firing started.
#[test]
fn the_queue_deadline_starts_at_the_reservation_not_the_firing() {
    let rig = Rig::open();
    let publishing = rig.state();
    let publishing_row_version = rig.store.load(SESSION).unwrap().row_version.unwrap();
    let mut validated = validated_range(2, 4);
    validated.extraction = ExtractionOutcome::Accepted { count: 2 };
    let target = rig.target();
    let decision = memory_reviewer_decision_before_publish(MemoryReviewerDecisionRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        publishing: &publishing,
        publishing_row_version,
        validated: &validated,
        aliases: &aliases(),
        pending: pending_publication(&validated),
        memory_reviewer_handoff: Some(&target),
        // The firing started ten minutes before its producer completed.
        failure_started_at_ms: t0() - 600_000,
        failure_backoff_at_ms: 0,
        completion_now_ms: t0,
    })
    .unwrap();
    assert!(decision.memory_reviewer_activation.is_some());
    assert_eq!(
        rig.reservation().queue_deadline_ms,
        t0() + MEMORY_REVIEWER_QUEUE_LIFETIME_MS
    );
}

#[test]
fn a_refused_publication_settles_only_the_reservation_it_attempted() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let attempted = rig.reservation();
    // Between this publication's refusal and its cleanup, another pass abandons firing 3, fires 4 over the same chunk, and firing 4 adopts the same job under its own reservation.
    struct MoveOnThenRefuse<'a>(&'a Rig);
    impl HistorySummarizerPublicationFence for MoveOnThenRefuse<'_> {
        fn publish(
            &self,
            _store: &MemoryStore,
            _request: memory_store::HistorySummarizerPublishRequest<'_>,
        ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerPublishError>
        {
            let rig = self.0;
            rig.persist(abandon_with_detail(
                &rig.state(),
                t0() + 1,
                Some("crash".to_string()),
            ));
            rig.persist(next_publishing_firing(rig, 2, 4));
            let _ = activation(rig.handoff(t0() + 2).unwrap());
            Err(HistorySummarizerPublishError::FenceRejected {
                reason: "stale".to_string(),
            })
        }
    }
    let adopted_before = rig.reservation();
    assert_eq!(adopted_before.firing_seq, 3);
    let loaded = rig.store.load(SESSION).unwrap();
    let predicate = publish_predicate(&loaded.meta.history_summarizer).unwrap();
    let fence = MoveOnThenRefuse(&rig);
    let validated = validated_range(2, 4);
    let refused = publish_validated_chunk(
        &rig.store,
        ValidatedPublishRequest {
            session_id: SESSION,
            project_path: PROJECT,
            expected_row_version: loaded.row_version,
            expected_revert_epoch: 0,
            predicate: &predicate,
            observed_chunk_fingerprint: "fp",
            validated: &validated,
            collect_user_memory_candidates: false,
            publication_floor_ordinal: 5,
            chunk_transcript: "U: transcript",
            boundary_dates: &BTreeMap::new(),
            created_at_ms: t0() + 3,
            now_ms: t0() + 3,
            failure_backoff_at_ms: t0() + 60_000,
            publication_fence: Some(&fence),
            memory_reviewer_nonadmission: None,
            memory_reviewer_activation: Some(&prepared),
        },
    );
    assert!(
        matches!(
            refused,
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::FenceRejected { .. }
            ))
        ),
        "{refused:?}"
    );
    // Firing 4 owns the job now: its reservation, retained publication, and the job all stand.
    let later = rig.state();
    assert_eq!(later.state, HistorySummarizerPhase::Publishing);
    assert_eq!(later.firing_seq, 4);
    let adopted = later
        .memory_reviewer_reservation
        .expect("firing 4 holds its reservation");
    assert_eq!(adopted.firing_seq, 4);
    assert_eq!(adopted.causal_identity, attempted.causal_identity);
    assert_eq!(rig.pending().map(|(firing_seq, _)| firing_seq), Some(4));
    assert_eq!(
        rig.job(&attempted.causal_identity).state,
        MemoryReviewerJobState::Reserved
    );
}

#[test]
fn the_retained_boundary_dates_are_those_of_the_validated_segments() {
    let mut boundary_dates = BTreeMap::new();
    for ordinal in 1..=20 {
        boundary_dates.insert(format!("m{ordinal}"), format!("2026-01-{ordinal:02}"));
    }
    let retained = retained_boundary_dates(&validated_range(2, 4), &boundary_dates);
    assert_eq!(
        retained,
        BTreeMap::from([
            ("m2".to_string(), "2026-01-02".to_string()),
            ("m4".to_string(), "2026-01-04".to_string()),
        ])
    );
    // A boundary message without a date is simply absent, as the publication reads it.
    let retained = retained_boundary_dates(&validated_range(2, 40), &boundary_dates);
    assert_eq!(
        retained,
        BTreeMap::from([("m2".to_string(), "2026-01-02".to_string())])
    );
}

/// A re-cut chunk that starts at another ordinal but yields the same facts adopts the orphaned reservation and reads the sealed subject under the binding it was staged with: the job's ordinal, not the adopting firing's.
#[test]
fn a_recut_firing_adopts_the_reservation_under_the_staged_binding() {
    let rig = Rig::open();
    let first = activation(rig.handoff(t0()).unwrap());
    let orphaned = rig.reservation();
    rig.persist(abandon_with_detail(
        &rig.state(),
        t0() + 1,
        Some("crash".to_string()),
    ));
    // Firing 4 re-cuts the chunk to 3..=4 and extracts the same facts from the same messages.
    rig.persist(next_publishing_firing(&rig, 3, 4));
    let adopted = activation(rig.handoff(t0() + 10).unwrap());
    assert_eq!(adopted.causal_identity, first.causal_identity);
    assert_eq!(adopted.producer.firing_id, format!("{}#4", rig_key()));
    assert_eq!(
        adopted.producer.ordinal, 2,
        "the job keeps the ordinal the subject was staged under"
    );
    assert!(rig.read_subject(&orphaned, t0() + 11).is_ok());
    let result = rig
        .publish_range(Some(&adopted), None, t0() + 12, 3, 4)
        .unwrap();
    assert_eq!(
        result.memory_reviewer_activation,
        Some(MemoryReviewerActivationOutcome::Activated)
    );
}

#[test]
fn an_unpublishable_reservation_settled_past_its_deadline_records_expiry_and_frees_the_firing() {
    // The retained payload cannot be read and the queue deadline has passed: the job closes as expired, the way the sweep would close it, and the firing returns to Idle in the same pass.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    rig.retain(
        &reservation,
        &memory_store::PendingPublication {
            validated_json: r#"{"schema":"a shape this daemon does not read"}"#.to_string(),
            ..pending_publication(&validated_range(2, 4))
        },
    );
    let late = reservation.queue_deadline_ms + 1;
    let outcome = republish_reserved(RepublishRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        memory_reviewer_handoff: None,
        now_ms: late,
        failure_backoff_at_ms: late + 60_000,
        publication_fence: None,
        collect_user_memory_candidates: false,
        memory_enabled: true,
    });
    assert!(
        matches!(outcome, Ok(RepublishOutcome::Settled)),
        "{outcome:?}"
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Expired)
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
}

#[test]
fn a_stale_retain_leaves_a_later_firing_untouched() {
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    // The pass snapshotted firing 3; before its refusal is handled, another path abandons firing 3, fires 4 over the same chunk, and firing 4 adopts the job under its own reservation.
    struct MoveOnThenRetire<'a>(&'a Rig);
    impl HistorySummarizerPublicationFence for MoveOnThenRetire<'_> {
        fn publish(
            &self,
            _store: &MemoryStore,
            _request: memory_store::HistorySummarizerPublishRequest<'_>,
        ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerPublishError>
        {
            let rig = self.0;
            rig.persist(abandon_with_detail(&rig.state(), t0() + 1, None));
            rig.persist(next_publishing_firing(rig, 2, 4));
            let _ = activation(rig.handoff(t0() + 2).unwrap());
            Err(HistorySummarizerPublishError::CallerFenceRejected {
                reason: "snapshot retired".to_string(),
            })
        }
    }
    let target = rig.target();
    let fence = MoveOnThenRetire(&rig);
    let outcome = republish_reserved(RepublishRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        memory_reviewer_handoff: Some(&target),
        now_ms: t0() + 3,
        failure_backoff_at_ms: t0() + 3 + 60_000,
        publication_fence: Some(&fence),
        collect_user_memory_candidates: false,
        memory_enabled: true,
    })
    .unwrap();
    assert_eq!(outcome, RepublishOutcome::Retained);
    // Firing 4's state carries no failure or backoff the stale pass for firing 3 decided.
    let later = rig.state();
    assert_eq!(later.firing_seq, 4);
    assert_eq!(later.state, HistorySummarizerPhase::Publishing);
    assert_eq!(later.last_failure, None);
    assert_eq!(later.failure_backoff_at_ms, None);
}

#[test]
fn a_republication_under_other_policy_versions_settles_instead_of_reserving_a_second_job() {
    use crate::memory_reviewer::broker::QuestionTemplate;

    // The firing reserved and retained under a previous step schema; the daemon that recovers it computes a different causal identity for the same subject. The retained output cannot publish under the recorded reservation, so it settles: no second job is reserved for the recovering daemon's policy.
    let rig = Rig::open();
    let reference = rig.staged_reference();
    let previous_policy = CausalInputs {
        target: ReviewTarget::StagedSubject {
            kernel_incarnation: rig.kernel_incarnation.clone(),
            candidate_id: reference.candidate_id.clone(),
            payload_digest: reference.payload_digest.clone(),
        },
        question_template: QuestionTemplate::ExtractedFacts.id().to_string(),
        signals: Vec::new(),
        required_evidence: Vec::new(),
        policy_versions: BTreeMap::from([("step_schema".to_string(), "previous".to_string())]),
    };
    let producer = ProducerBinding {
        producer: PRODUCER.to_string(),
        firing_id: format!("{}#3", rig_key()),
        ordinal: 2,
    };
    let previous = match rig
        .store
        .reserve_memory_reviewer_job(PROJECT, &producer, &previous_policy, t0())
        .unwrap()
    {
        memory_store::memory_reviewer_jobs::ReserveOutcome::Reserved(job) => job,
        other => panic!("{other:?}"),
    };
    rig.retain(
        &memory_store::MemoryReviewerReservation {
            firing_seq: 3,
            causal_identity: previous.causal_identity.clone(),
            candidate_id: reference.candidate_id.clone(),
            payload_digest: reference.payload_digest.clone(),
            kernel_incarnation: rig.kernel_incarnation.clone(),
            queue_deadline_ms: previous.queue_deadline_ms,
        },
        &pending_publication(&validated_range(2, 4)),
    );
    let headroom_before = rig.store.memory_reviewer_headroom(PROJECT).unwrap();
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), t0() + 1),
        RepublishOutcome::Settled
    );
    assert_eq!(
        rig.job(&previous.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    assert_eq!(
        rig.store
            .memory_reviewer_headroom(PROJECT)
            .unwrap()
            .pending_jobs,
        headroom_before.pending_jobs - 1,
        "the previous policy's job closed and no job was reserved for the current one"
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
}

#[test]
fn a_publication_without_an_activation_drops_a_retained_publication_no_reservation_names() {
    // Firing 3 was abandoned with its reservation retained; firing 4 fires (dropping the reservation pointer) and publishes with nothing to hand off. The retained row of firing 3 has no consumer left and goes with that publication.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    rig.persist(abandon_with_detail(
        &rig.state(),
        t0() + 1,
        Some("crash".to_string()),
    ));
    assert_eq!(rig.pending().map(|(firing_seq, _)| firing_seq), Some(3));
    rig.persist(next_publishing_firing(&rig, 5, 6));
    assert_eq!(rig.state().memory_reviewer_reservation, None);
    rig.publish_range(None, None, t0() + 10, 5, 6).unwrap();
    assert_eq!(rig.state().state, HistorySummarizerPhase::Idle);
    assert_eq!(rig.pending(), None);
}

#[test]
fn a_handoff_failure_retains_only_the_firing_whose_handoff_failed() {
    let rig = Rig::open();
    let publishing = rig.state();
    let publishing_row_version = rig.store.load(SESSION).unwrap().row_version.unwrap();
    // Another path moved the session on: firing 3 was abandoned, firing 4 fired and holds its own reservation with its retained publication.
    rig.persist(abandon_with_detail(&rig.state(), t0() + 1, None));
    rig.persist(next_publishing_firing(&rig, 5, 6));
    rig.retain(
        &later_reservation(),
        &pending_publication(&validated_range(5, 6)),
    );
    let before = rig.state();
    assert_eq!(before.firing_seq, 4);
    assert_eq!(before.last_failure, None);
    // Firing 3's handoff fails at persistence: its row version is stale.
    let target = rig.target();
    let result = memory_reviewer_decision_before_publish(MemoryReviewerDecisionRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        publishing: &publishing,
        publishing_row_version,
        validated: &accepted_range(2, 4),
        aliases: &aliases(),
        pending: pending_publication(&validated_range(2, 4)),
        memory_reviewer_handoff: Some(&target),
        failure_started_at_ms: t0(),
        failure_backoff_at_ms: t0() + 60_000,
        completion_now_ms: t0,
    });
    assert!(
        matches!(
            result,
            Err(HistorySummarizerDriveError::MemoryReviewerHandoff(_))
        ),
        "{:?}",
        result.err()
    );
    // Firing 4 carries nothing of firing 3's failure.
    let after = rig.state();
    assert_eq!(after.firing_seq, 4);
    assert_eq!(after.state, HistorySummarizerPhase::Publishing);
    assert_eq!(after.memory_reviewer_reservation, Some(later_reservation()));
    assert_eq!(after.last_failure, None);
    assert_eq!(after.failure_backoff_at_ms, None);
    assert_eq!(
        after.consecutive_publish_failures,
        before.consecutive_publish_failures
    );
}

#[test]
fn a_republication_with_memory_disabled_settles_instead_of_activating() {
    // Memory was disabled after the firing retained its accepted facts; recovery does not hand them to the MemoryReviewer. The reservation settles and the firing refires under the current configuration.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let target = rig.target();
    let outcome = republish_reserved(RepublishRequest {
        store: &rig.store,
        session_id: SESSION,
        project_path: PROJECT,
        memory_reviewer_handoff: Some(&target),
        now_ms: t0() + 1,
        failure_backoff_at_ms: t0() + 60_000,
        publication_fence: None,
        collect_user_memory_candidates: false,
        memory_enabled: false,
    })
    .unwrap();
    assert_eq!(outcome, RepublishOutcome::Settled);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Nonadmitted)
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
}

#[test]
fn a_firing_holding_its_reservation_does_not_take_back_a_job_another_firing_adopted() {
    // Firing 3 holds its reservation; before it republishes, firing 4 adopted the same job. The handoff for firing 3 reuses the job only while it is still bound to firing 3: it does not rebind it back, and answers Settled.
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let holder = rig.state();
    assert!(holder.holds_reservation());
    let adopter = ProducerBinding {
        firing_id: format!("{}#4", rig_key()),
        ..prepared.producer.clone()
    };
    rig.store
        .rebind_reserved_memory_reviewer_job(PROJECT, &prepared.causal_identity, &adopter, t0() + 1)
        .unwrap();
    let handoff = reserve_and_stage(
        &rig.target(),
        &HandoffRequest {
            store: &rig.store,
            project: PROJECT,
            session_id: SESSION,
            firing: &holder,
            facts: &facts(),
            aliases: &aliases(),
            now_ms: t0() + 2,
        },
        |_| Ok(0),
    )
    .unwrap();
    assert!(matches!(handoff, Handoff::Settled), "{handoff:?}");
    assert_eq!(rig.job(&prepared.causal_identity).producer, adopter);
}

#[test]
fn a_recovery_clocked_before_the_deadline_still_publishes_a_job_the_sweep_expired() {
    // The pass captured its clock just before the deadline; by the time it looks the job up, the sweep has closed it as expired. The terminal state proves the deadline passed: the retained history publishes through the expiry path instead of being discarded.
    let rig = Rig::open();
    let _ = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let (jobs, _) = rig
        .store
        .expire_memory_reviewer_work(reservation.queue_deadline_ms + 1)
        .unwrap();
    assert_eq!(jobs, 1);
    let target = rig.target();
    assert_eq!(
        rig.republish(Some(&target), reservation.queue_deadline_ms - 1),
        RepublishOutcome::Published
    );
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        MemoryReviewerJobState::Terminal(MemoryReviewerJobOutcome::Expired)
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.memory_reviewer_reservation, None);
    assert_eq!(rig.pending(), None);
}
