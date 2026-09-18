//! The History Summarizer's reserve, stage, activate handoff against a real Kernel and Memory Store (KTD3, #595): the reservation exists before anything is staged and is recorded in the producer's durable state; staging and sealing converge after an interruption at every boundary; the fenced publication activates the job with its history or commits neither; an expired reservation is closed as expired by a late publication; identical inputs neither duplicate a job nor reopen a settled one; capacity refusal before the reservation is the nonadmission the publication records; and the staged subject reports one origin per cited block with exact ranges.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::curator::handoff::{
    Handoff, HandoffError, HandoffRequest, HandoffTarget, PRODUCER, SUBJECT_SOURCE_KIND,
    reserve_and_stage, review_binding, review_subject,
};
use crate::history_summarizer::{ValidatedPublishRequest, publish_validated_chunk};
use crate::history_summarizer_citations::{Citation, FrozenAlias, FrozenAliasTable};
use crate::history_summarizer_validate::{FactCandidate, ValidatedChunk, ValidatedHistorySegment};
use kernel::{
    ByteRange, KernelStore, ReviewPayload, ReviewReadError, ReviewReadRefusal,
    ReviewStagedReference,
};
use memory_store::HistorySummarizerPublishPredicate;
use memory_store::curator_jobs::{
    CURATOR_QUEUE_LIFETIME_MS, CausalInputs, CuratorJobOutcome, CuratorJobState,
    MAX_PENDING_CURATOR_JOBS_PER_PROJECT, ProducerBinding, ReviewTarget,
};
use memory_store::{
    BlockIdentity, CuratorActivationOutcome, CuratorNonadmissionCode, HistorySegmentSetGeneration,
    HistorySummarizerChunkRange, HistorySummarizerDurableState, HistorySummarizerPhase,
    HistorySummarizerPublishError, HistorySummarizerSelectedMessageIdentity, MemoryStore,
    ModuleMeta,
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
    fn reservation(&self) -> memory_store::CuratorReservation {
        self.state()
            .curator_reservation
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
        observe: impl FnOnce(&memory_store::CuratorReservation),
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
                let row_version = self.store.load(SESSION).unwrap().row_version.unwrap();
                Ok(self
                    .store
                    .record_curator_reservation(
                        SESSION,
                        row_version,
                        reservation,
                        &memory_store::PendingPublication {
                            validated_json: serde_json::to_string(&validated_range(2, 4)).unwrap(),
                            aliases_json: serde_json::to_string(&aliases()).unwrap(),
                            chunk_transcript: "U: transcript".to_string(),
                            boundary_dates: BTreeMap::new(),
                            publication_floor_ordinal: 5,
                            collect_user_memory_candidates: false,
                        },
                    )
                    .unwrap())
            },
        )
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
            curator_handoff: target,
            now_ms,
            failure_backoff_at_ms: now_ms + 60_000,
            publication_fence: None,
        })
        .unwrap()
    }

    /// The reference the handoff stages the accepted facts under.
    fn staged_reference(&self) -> ReviewStagedReference {
        let payload = ReviewPayload::Subject(review_subject(&facts(), &aliases()).unwrap());
        let payload_digest = payload.digest().unwrap();
        ReviewStagedReference {
            database_incarnation_id: self.kernel_incarnation.clone(),
            candidate_id: format!("hs-{SESSION}-{}", &payload_digest[..32]),
            payload_digest,
        }
    }

    fn publish(
        &self,
        activation: Option<&PreparedActivation>,
        nonadmission: Option<CuratorNonadmissionCode>,
        now_ms: i64,
    ) -> Result<memory_store::HistorySummarizerPublishResult, HistorySummarizerStateError> {
        self.publish_range(activation, nonadmission, now_ms, 2, 4)
    }

    fn publish_range(
        &self,
        activation: Option<&PreparedActivation>,
        nonadmission: Option<CuratorNonadmissionCode>,
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
        nonadmission: Option<CuratorNonadmissionCode>,
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
                failure_backoff_at_ms: now_ms + 60_000,
                publication_fence: None,
                curator_nonadmission: nonadmission,
                curator_activation: activation,
            },
        )
    }

    fn job(&self, causal_identity: &str) -> memory_store::curator_jobs::CuratorJob {
        self.store
            .lookup_curator_job(PROJECT, causal_identity)
            .unwrap()
            .expect("the job row exists")
    }

    fn read_subject(
        &self,
        reservation: &memory_store::CuratorReservation,
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
                reservation.firing_seq,
                &reservation.causal_identity,
            ),
            now_ms,
        )
    }
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
            origin_history_segment_index: None,
            citations: vec![
                citation("s1", 0, 11),
                citation("s1", 12, 35),
                citation("s2", 0, 22),
            ],
        },
        FactCandidate {
            category: "CONFIG_VALUES".to_string(),
            content: "The package manager is bun.".to_string(),
            origin_history_segment_index: None,
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

fn activation(handoff: Handoff) -> PreparedActivation {
    match handoff {
        Handoff::Activate(prepared) => *prepared,
        other => panic!("expected an activation, got {other:?}"),
    }
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
                CuratorJobState::Reserved
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
        .curator_reservation
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
        t0() + CURATOR_QUEUE_LIFETIME_MS
    );
    let job = rig.job(&reservation.causal_identity);
    assert_eq!(job.state, CuratorJobState::Reserved);
    assert_eq!(
        job.producer,
        ProducerBinding {
            producer: PRODUCER.to_string(),
            firing_id: format!("{SESSION}#3"),
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
    // The subject is sealed and reads back under the binding the coordinator reconstructs; the binding cites the session's chunk at this firing.
    let row = rig.read_subject(&reservation, t0() + 1).unwrap();
    assert_eq!(row.binding.subject_source.source_kind, SUBJECT_SOURCE_KIND);
    assert_eq!(row.binding.subject_source.source_id, SESSION);
    assert_eq!(row.binding.subject_source.source_revision, 3);
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
        result.curator_activation,
        Some(CuratorActivationOutcome::Activated)
    );
    assert_eq!(result.curator_nonadmission_count, 0);
    let job = rig.job(&reservation.causal_identity);
    match job.state {
        CuratorJobState::Ready(input) => {
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
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
    assert_eq!(after.meta.history_summarizer.curator_nonadmission.count, 0);
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
    assert_eq!(rig.store.ready_curator_jobs(PROJECT, 8).unwrap().len(), 1);
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
    assert_eq!(rig.state().curator_reservation, None);
    let rows_after_window_1 = rig.store.curator_headroom(PROJECT).unwrap();
    // Window 2: the state is written but staging never ran (simulated by a fresh call that finds its own reservation).
    let prepared = activation(rig.handoff(t0() + 10).unwrap());
    assert_eq!(
        rig.store.curator_headroom(PROJECT).unwrap(),
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
                    HistorySummarizerPublishError::CuratorActivation(_)
                )
            )
        ),
        "{refused:?}"
    );
    assert_eq!(
        rig.job(&prepared.causal_identity).state,
        CuratorJobState::Reserved
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    let retained = rig.state();
    assert_eq!(retained.state, HistorySummarizerPhase::Publishing);
    assert_eq!(retained.curator_reservation, Some(reservation.clone()));
    assert_eq!(retained.curator_nonadmission.count, 0);
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
        result.curator_activation,
        Some(CuratorActivationOutcome::Activated)
    );
    assert_eq!(rig.store.ready_curator_jobs(PROJECT, 8).unwrap().len(), 1);
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
        CuratorJobState::Terminal(CuratorJobOutcome::Nonadmitted)
    );
    let after = rig.store.load(SESSION).unwrap();
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(after.meta.publication_floor_ordinal, None);
    assert_eq!(
        after.meta.history_summarizer.state,
        HistorySummarizerPhase::Idle
    );
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
    assert_eq!(rig.pending(), None);
    assert_eq!(after.meta.history_summarizer.curator_nonadmission.count, 0);
    assert!(rig.store.ready_curator_jobs(PROJECT, 8).unwrap().is_empty());
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
        CuratorJobState::Terminal(CuratorJobOutcome::Nonadmitted)
    );
    assert_eq!(rig.state().curator_reservation, None);
}

#[test]
fn a_late_publication_records_expiry_and_never_resurrects_the_reservation() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let late = t0() + CURATOR_QUEUE_LIFETIME_MS;
    let result = rig.publish(Some(&prepared), None, late).unwrap();
    assert_eq!(
        result.curator_activation,
        Some(CuratorActivationOutcome::Expired)
    );
    let job = rig.job(&prepared.causal_identity);
    assert_eq!(
        job.state,
        CuratorJobState::Terminal(CuratorJobOutcome::Expired)
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
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
    assert_eq!(
        after.meta.history_summarizer.curator_nonadmission.count, 0,
        "expiry after reservation is a terminal outcome, not a nonadmission"
    );
    // The staged subject is past its deadline too.
    assert!(matches!(
        rig.read_subject(&reservation, late),
        Err(ReviewReadError::Refused(ReviewReadRefusal::Expired))
    ));
    assert!(rig.store.ready_curator_jobs(PROJECT, 8).unwrap().is_empty());
}

#[test]
fn identical_inputs_neither_duplicate_a_job_nor_reopen_a_settled_one() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    rig.publish(Some(&prepared), None, t0() + 1).unwrap();
    // A later firing with the same accepted facts finds the Ready row and hands off nothing.
    rig.persist(publishing_state(4));
    assert_eq!(rig.handoff(t0() + 2).unwrap(), Handoff::Settled);
    assert_eq!(rig.state().curator_reservation, None);
    assert_eq!(rig.store.ready_curator_jobs(PROJECT, 8).unwrap().len(), 1);
    // Once the job is terminal, the same inputs stay settled.
    rig.store
        .finish_curator_job(
            PROJECT,
            &prepared.causal_identity,
            CuratorJobOutcome::Completed,
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
        publication.curator_nonadmission_count, 0,
        "settled inputs are not a nonadmission"
    );
}

#[test]
fn capacity_refusal_before_the_reservation_is_the_recorded_nonadmission() {
    let rig = Rig::open();
    // Fill the project's pending capacity with unrelated reservations.
    for index in 0..MAX_PENDING_CURATOR_JOBS_PER_PROJECT {
        rig.store
            .reserve_curator_job(
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
        Handoff::Nonadmission(CuratorNonadmissionCode::CapacityFull)
    );
    assert_eq!(
        rig.state().curator_reservation,
        None,
        "no reservation was persisted"
    );
    // No subject was staged for it.
    let missing = rig.kernel.read_review_input(
        &rig.staged_reference(),
        &review_binding(PROJECT_DIGEST, DOMAIN, SESSION, 3, "job"),
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
        .publish(None, Some(CuratorNonadmissionCode::CapacityFull), t0() + 2)
        .unwrap();
    assert_eq!(published.curator_nonadmission_count, 1);
    let state = rig.state();
    assert_eq!(
        state.curator_nonadmission.latest.map(|latest| latest.code),
        Some(CuratorNonadmissionCode::CapacityFull)
    );
    assert_eq!(
        state
            .curator_nonadmission
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
            curator_handoff: Some(&target),
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
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
    assert_eq!(after.meta.history_summarizer.curator_nonadmission.count, 0);
    let ready = rig.store.ready_curator_jobs(PROJECT, 8).unwrap();
    assert_eq!(ready.len(), 1);
    let job = &ready[0];
    assert_eq!(job.producer.firing_id, format!("{SESSION}#3"));
    let CuratorJobState::Ready(input) = &job.state else {
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
            &review_binding(PROJECT_DIGEST, DOMAIN, SESSION, 3, &job.causal_identity),
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
    next.curator_nonadmission = after.meta.history_summarizer.curator_nonadmission;
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
    assert_eq!(state.curator_nonadmission.count, 1);
    assert_eq!(
        state.curator_nonadmission.latest.map(|latest| latest.code),
        Some(CuratorNonadmissionCode::FactSetRejected {
            failure: ExtractionFailure::UnknownAlias,
        })
    );
    assert_eq!(state.curator_reservation, None);
    assert_eq!(rig.store.ready_curator_jobs(PROJECT, 8).unwrap().len(), 1);
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
                HistorySummarizerPublishError::CuratorActivation(
                    memory_store::curator_jobs::CuratorJobRefusal::ProducerMismatch
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
        state.meta.history_summarizer.curator_reservation,
        Some(reservation.clone())
    );
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        CuratorJobState::Reserved
    );
    assert!(rig.read_subject(&reservation, t0() + 2).is_ok());

    // A publication that names no activation while a reservation is recorded is refused the same way; so is one whose activation names another job.
    let refused = rig.publish(None, None, t0() + 3);
    assert!(
        matches!(
            refused,
            Err(HistorySummarizerStateError::Publish(
                HistorySummarizerPublishError::CuratorActivation(
                    memory_store::curator_jobs::CuratorJobRefusal::InvalidRequest
                )
            ))
        ),
        "{refused:?}"
    );
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        CuratorJobState::Reserved
    );

    // The matching activation then publishes once.
    let result = rig.publish(Some(&prepared), None, t0() + 4).unwrap();
    assert_eq!(
        result.curator_activation,
        Some(CuratorActivationOutcome::Activated)
    );
    assert_eq!(rig.store.load_history_segments(SESSION).unwrap().len(), 1);
}

#[test]
fn a_reservation_the_sweep_already_expired_reads_as_expired_at_late_publication() {
    let rig = Rig::open();
    let prepared = activation(rig.handoff(t0()).unwrap());
    let reservation = rig.reservation();
    let late = t0() + CURATOR_QUEUE_LIFETIME_MS + 1;
    let (jobs, _) = rig.store.expire_curator_work(late).unwrap();
    assert_eq!(jobs, 1);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        CuratorJobState::Terminal(CuratorJobOutcome::Expired)
    );
    let result = rig.publish(Some(&prepared), None, late).unwrap();
    assert_eq!(
        result.curator_activation,
        Some(CuratorActivationOutcome::Expired)
    );
    let after = rig.store.load(SESSION).unwrap();
    assert_eq!(after.meta.publication_floor_ordinal, Some(5));
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
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
    assert_eq!(rig.state().curator_reservation, Some(reservation.clone()));
    assert!(rig.store.load_history_segments(SESSION).unwrap().is_empty());
    // With it, the retained publication commits and activates the same job.
    let target = rig.target();
    let outcome = rig.republish(Some(&target), t0() + 2);
    assert_eq!(outcome, RepublishOutcome::Published);
    let job = rig.job(&reservation.causal_identity);
    assert!(
        matches!(job.state, CuratorJobState::Ready(_)),
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
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
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
        CuratorJobState::Terminal(CuratorJobOutcome::Nonadmitted)
    );
    let after = rig.state();
    assert_eq!(after.state, HistorySummarizerPhase::Idle);
    assert_eq!(after.curator_reservation, None);
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
    let late = t0() + CURATOR_QUEUE_LIFETIME_MS + 1;
    let target = rig.target();
    let outcome = rig.republish(Some(&target), late);
    assert_eq!(outcome, RepublishOutcome::Published);
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        CuratorJobState::Terminal(CuratorJobOutcome::Expired)
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
        curator_handoff: Some(&target),
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
    let reservation = retained.curator_reservation.clone().expect("retained");
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
        CuratorJobState::Reserved
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
    assert_eq!(after.meta.history_summarizer.curator_reservation, None);
    assert_eq!(rig.pending(), None);
    let segments = rig.store.load_history_segments(SESSION).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!((segments[0].start_message, segments[0].end_message), (2, 3));
    assert!(matches!(
        rig.job(&reservation.causal_identity).state,
        CuratorJobState::Ready(_)
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
    stale.curator_reservation = Some(reservation.clone());
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
    assert_eq!(fired.curator_reservation, None);
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
    assert_eq!(state.curator_reservation, Some(reservation.clone()));
    assert!(rig.pending().is_some());
    assert_eq!(
        rig.job(&reservation.causal_identity).state,
        CuratorJobState::Reserved
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
        CuratorJobState::Ready(_)
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
        CuratorJobState::Ready(_)
    ));
}
