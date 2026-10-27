use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc;

use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactHandle, CommitIntent, DecisionPayload, DecisionSpec,
    DomainSpec, EventKind, KernelStore, LiveDescriptor, SourceClass, TaintClass,
};
use memory_store::contradiction_candidates::StoredCandidate;

use super::*;
use crate::contradiction_candidates::pass::{Descriptor, PassBounds, score_publication};
use crate::contradiction_candidates::{Candidates, Count as CandidateCount};

const SESSION: &str = "ses";
const TOPIC: &str = "kernel snapshot ledger quorum lease";

fn candidate_config(capture: bool, candidates: bool) -> DaemonConfig {
    let mut config = default_test_config();
    config.model_chain.clear();
    config.source_capture_enabled = capture;
    config.contradiction_candidates_enabled = candidates;
    config
}

async fn candidate_handler(
    config: DaemonConfig,
) -> (Handler, Arc<MemoryStore>, tempfile::TempDir, PathBuf) {
    let (handler, store, dir, project) =
        handler_with_store_and_kernel(Arc::new(ProducerState::default()), config.clone()).await;
    let mut bound = binding_with_harness(project.to_str().unwrap(), "opencode", SESSION);
    bound.config = config;
    handler.bind_route(test_route(7), bound);
    (handler, store, dir, project)
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "candidate-pass-test".to_owned(),
        operation_key: key.to_owned(),
        request_digest: kernel::source_identity::identity_digest(key.as_bytes()),
        actor: "test".to_owned(),
        cause: "fixture".to_owned(),
    }
}

fn admission(object_id: &str, kind: EventKind) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_owned()),
        source_class: Some(SourceClass::TrustedLocalCode),
        taint_class: Some(TaintClass::CurrentCode),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".to_owned(),
        },
    }
}

fn memory_domain() -> DomainSpec {
    DomainSpec {
        domain_id: crate::canonical_memory::MEMORY_DOMAIN_ID.to_owned(),
        object_id: "memory-domain".to_owned(),
        name: "memory".to_owned(),
        source_kind: "fixture".to_owned(),
        source_id: "memory".to_owned(),
        source_revision: 1,
        sensitivity: kernel::Sensitivity::Normal,
    }
}

fn seed(
    kernel: &KernelStore,
    project: &kernel_routes::ProjectBinding,
    decisions: &[(&str, &str, EventKind)],
) {
    kernel
        .commit(intent(&format!("seed-{}", decisions[0].0)), |envelope| {
            if !envelope.domain_exists(crate::canonical_memory::MEMORY_DOMAIN_ID)? {
                envelope.insert_domain(memory_domain())?;
            }
            let scope = project.scope_spec(crate::canonical_memory::MEMORY_DOMAIN_ID);
            if envelope.scope_terms(&scope.scope_id)?.is_none() {
                envelope.insert_scope(scope)?;
            }
            for (name, summary, _) in decisions {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("decision-{name}"),
                    object_id: format!("object-{name}"),
                    domain_id: crate::canonical_memory::MEMORY_DOMAIN_ID.to_owned(),
                    proposition_id: None,
                    scope_id: Some(project.scope_id()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "architecture".to_owned(),
                    payload: DecisionPayload {
                        summary: (*summary).to_owned(),
                        rationale: "fixture".to_owned(),
                    },
                    source_kind: "fixture".to_owned(),
                    source_id: format!("lineage-{name}"),
                    source_revision: 1,
                    sensitivity: kernel::Sensitivity::Normal,
                })?;
                envelope
                    .record_admission(admission(&format!("object-{name}"), EventKind::Other))?;
            }
            Ok(String::new())
        })
        .unwrap();
    for (name, _, kind) in decisions {
        if *kind != EventKind::Other {
            kernel
                .commit(intent(&format!("event-{name}")), |envelope| {
                    envelope.record_admission(admission(&format!("object-{name}"), *kind))?;
                    Ok(String::new())
                })
                .unwrap();
        }
    }
}

fn opencode_user(id: &str, created: u64, text: &str) -> Value {
    json!({
        "info": { "id": id, "sessionID": SESSION, "role": "user", "time": { "created": created } },
        "parts": [{ "type": "text", "text": text }],
    })
}

fn window(index: u64, text: &str) -> Vec<Value> {
    vec![opencode_user(
        &format!("msg_{index:03}"),
        1_700_000_000_000 + index * 1_000,
        text,
    )]
}

fn publish(handler: &Handler, project: &Path, window: &[Value], candidates: bool) {
    let offer = source_capture::Offer {
        profile: SerializerProfile::OpencodeAiSdk,
        session_id: SESSION.to_owned(),
        project: kernel_routes::ProjectBinding::new(project),
        window: window.iter().cloned().map(Arc::new).collect(),
        epoch: handler.source_capture.epoch(),
        documents: None,
        candidates,
    };
    handler
        .capture_after_response(offer)
        .expect("a charged window")
        .run();
}

async fn settle(handler: &Handler) {
    for _ in 0..1_000 {
        let idle =
            || handler.source_capture.is_idle() && handler.contradiction_candidates.is_idle();
        if idle() {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if idle() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the publication or its pass never finished");
}

async fn until(handler: &Handler, name: &str, value: u64) {
    for _ in 0..1_000 {
        if counts(handler)[name] == value {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{name} never reached {value}: {}", counts(handler));
}

fn counts(handler: &Handler) -> Value {
    handler.contradiction_candidates.report()
}

fn capture_report(handler: &Handler) -> Value {
    handler.source_capture.report(false, now_ms())
}

fn rows(store: &MemoryStore) -> Vec<StoredCandidate> {
    let mut rows: Vec<StoredCandidate> = store
        .candidate_sweep_page(0, 100_000)
        .unwrap()
        .into_iter()
        .map(|(_, row)| row)
        .collect();
    rows.sort_by(|a, b| {
        (&a.evidence_id, &a.decision_object_id).cmp(&(&b.evidence_id, &b.decision_object_id))
    });
    rows
}

fn kernel_of(handler: &Handler) -> Arc<KernelStore> {
    handler.kernel.kernel_store().unwrap()
}

fn live_messages(kernel: &KernelStore) -> Vec<LiveDescriptor> {
    kernel
        .live_source_descriptors(
            OccurrenceClass::Messages,
            kernel.tip().unwrap(),
            None,
            NonZeroUsize::new(64).unwrap(),
            &kernel::applicability::EvalBudget::unbounded(),
        )
        .unwrap()
        .rows
}

fn retained_text(kernel: &KernelStore, row: &LiveDescriptor) -> String {
    let handle = ArtifactHandle {
        digest: row.detail.artifact_digest.clone(),
        evidence_id: row.detail.evidence_id.clone(),
    };
    String::from_utf8(kernel.read_artifact(&handle).unwrap()).unwrap()
}

fn park(handler: &Handler) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    *handler.contradiction_candidates.before_pass.lock().unwrap() = Some(Arc::new(move || {
        let _ = entered_tx.send(());
        let _ = release_rx.lock().unwrap().recv();
    }));
    (entered, release)
}

fn unhook(handler: &Handler) {
    *handler.contradiction_candidates.before_pass.lock().unwrap() = None;
}

fn entered(receiver: &mpsc::Receiver<()>) {
    receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the pass reached its hook");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_route_without_consent_offers_no_pass_and_writes_no_row() {
    let (handler, store, _dir, project) = candidate_handler(candidate_config(true, false)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    seed(
        &kernel_of(&handler),
        &binding,
        &[("one", TOPIC, EventKind::Other)],
    );
    for index in 0..3 {
        publish(
            &handler,
            &project,
            &window(index, &format!("{TOPIC} note {index}")),
            false,
        );
        settle(&handler).await;
    }
    assert_eq!(capture_report(&handler)["units_published"], 3);
    assert!(
        counts(&handler)
            .as_object()
            .unwrap()
            .values()
            .all(|value| value == 0),
        "{}",
        counts(&handler)
    );
    assert!(rows(&store).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_starts_only_after_the_job_releases_the_slot_and_the_gate() {
    let (handler, store, _dir, project) = candidate_handler(candidate_config(true, true)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    seed(
        &kernel_of(&handler),
        &binding,
        &[("one", TOPIC, EventKind::Other)],
    );
    let (publishing_tx, publishing) = mpsc::channel();
    let (go, go_rx) = mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        let _ = publishing_tx.send(());
        let _ = go_rx.lock().unwrap().recv();
    }));
    let gate = Arc::clone(&handler.capture_commit_gate);
    let capture = Arc::clone(&handler.source_capture);
    let observed = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&observed);
    *handler.contradiction_candidates.before_pass.lock().unwrap() = Some(Arc::new(move || {
        *seen.lock().unwrap() = Some((capture.is_idle(), gate.try_lock().is_ok()));
    }));
    let scoring_gate = Arc::clone(&handler.capture_commit_gate);
    let scoring = Arc::new(Mutex::new(Vec::new()));
    let scored = Arc::clone(&scoring);
    *handler
        .contradiction_candidates
        .before_descriptor
        .lock()
        .unwrap() = Some(Arc::new(move |_| {
        scored.lock().unwrap().push(scoring_gate.try_lock().is_ok());
    }));
    publish(
        &handler,
        &project,
        &window(0, &format!("{TOPIC} first")),
        true,
    );
    publishing
        .recv_timeout(Duration::from_secs(10))
        .expect("the job reached its publish hook");
    assert_eq!(counts(&handler)["offered"], 0);
    assert!(rows(&store).is_empty());
    go.send(()).unwrap();
    settle(&handler).await;
    *handler.source_capture.before_publish.lock().unwrap() = None;
    unhook(&handler);
    assert_eq!(
        *observed.lock().unwrap(),
        Some((true, true)),
        "the pass runs with the slot released and the commit gate free"
    );
    let block = counts(&handler);
    for (name, value) in [
        ("offered", 1),
        ("admitted", 1),
        ("completed", 1),
        ("active", 0),
    ] {
        assert_eq!(block[name], value, "{name}: {block}");
    }
    assert_eq!(rows(&store).len(), 1);
    publish(
        &handler,
        &project,
        &window(0, &format!("{TOPIC} first")),
        true,
    );
    settle(&handler).await;
    assert_eq!(
        counts(&handler)["offered"],
        1,
        "a repeated window is a memo hit, publishes nothing, and offers no pass"
    );
    assert_eq!(
        capture_report(&handler)["completed"],
        2,
        "each job still ends in exactly one terminal count"
    );
    let before = rows(&store);
    handler.source_capture.forget_memo_for_test();
    publish(
        &handler,
        &project,
        &window(0, &format!("{TOPIC} first")),
        true,
    );
    settle(&handler).await;
    assert!(capture_report(&handler)["units_replayed"].as_u64().unwrap() >= 1);
    assert_eq!(
        counts(&handler)["completed"],
        2,
        "a replayed unit is scored again"
    );
    let after = rows(&store);
    assert_eq!(after.len(), before.len());
    for (earlier, later) in before.iter().zip(&after) {
        assert_eq!(
            (&earlier.evidence_id, &earlier.decision_object_id),
            (&later.evidence_id, &later.decision_object_id)
        );
        assert!(later.publish_tip >= earlier.publish_tip);
    }
    *handler
        .contradiction_candidates
        .before_descriptor
        .lock()
        .unwrap() = None;
    let scoring = scoring.lock().unwrap();
    assert!(
        !scoring.is_empty() && scoring.iter().all(|free| *free),
        "the commit gate is free while each descriptor is scored"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_offer_while_a_pass_runs_is_refused_and_a_cancel_still_ends_the_pass() {
    let (handler, store, _dir, project) = candidate_handler(candidate_config(true, true)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    seed(
        &kernel_of(&handler),
        &binding,
        &[("one", TOPIC, EventKind::Other)],
    );
    let (reached, release) = park(&handler);
    publish(
        &handler,
        &project,
        &window(0, &format!("{TOPIC} first")),
        true,
    );
    entered(&reached);
    publish(
        &handler,
        &project,
        &window(1, &format!("{TOPIC} second")),
        true,
    );
    until(&handler, "refused_pass_busy", 1).await;
    assert_eq!(counts(&handler)["active"], 1);
    release.send(()).unwrap();
    settle(&handler).await;
    assert_eq!(
        rows(&store).len(),
        1,
        "the refused publication wrote nothing"
    );
    assert_eq!(counts(&handler)["active"], 0);
    unhook(&handler);

    let cancel = handler.cancel.clone();
    *handler
        .contradiction_candidates
        .before_descriptor
        .lock()
        .unwrap() = Some(Arc::new(move |index| {
        if index == 1 {
            cancel.cancel();
            // The async side raises the budget's interrupt once it observes the cancel.
            std::thread::sleep(Duration::from_millis(300));
        }
    }));
    let mut native = window(2, &format!("{TOPIC} third"));
    native.extend(window(3, &format!("{TOPIC} fourth")));
    native.extend(window(4, &format!("{TOPIC} fifth")));
    publish(&handler, &project, &native, true);
    until(&handler, "budget_exhausted", 1).await;
    until(&handler, "active", 0).await;
    *handler
        .contradiction_candidates
        .before_descriptor
        .lock()
        .unwrap() = None;
    assert_eq!(counts(&handler)["descriptors_unscored"], 2);
    assert_eq!(
        rows(&store).len(),
        2,
        "the cancelled pass wrote the rows of its first descriptor only"
    );
}

struct Core {
    dir: tempfile::TempDir,
    kernel: KernelStore,
    store: MemoryStore,
    project: kernel_routes::ProjectBinding,
    counts: Candidates,
}

impl Core {
    fn open(bounds: PassBounds) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("kernel")).unwrap();
        let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
        let store = MemoryStore::open_for_test(dir.path(), "pass");
        let project = kernel_routes::ProjectBinding::new(&dir.path().join("project"));
        kernel
            .commit(intent("domain"), |envelope| {
                envelope.insert_domain(memory_domain())?;
                Ok(String::new())
            })
            .unwrap();
        let counts = Candidates::default();
        *counts.test_bounds.lock().unwrap() = Some(bounds);
        Self {
            dir,
            kernel,
            store,
            project,
            counts,
        }
    }

    fn descriptor(&self, name: &str, text: &str) -> Descriptor {
        let payload = text.as_bytes().to_vec();
        let mut request = intent(&format!("evidence-{name}"));
        request.request_digest = kernel::source_identity::identity_digest(&payload);
        let handle = self
            .kernel
            .ingest_exact_artifact(kernel::ArtifactIngestRequest {
                intent: request,
                payload,
                evidence_id: format!("srcev:{name}"),
                object_id: format!("srcev-object:{name}"),
                object_kind: "evidence".to_owned(),
                domain_id: crate::canonical_memory::MEMORY_DOMAIN_ID.to_owned(),
                source_kind: "messages".to_owned(),
                source_id: format!("native/{name}"),
                source_revision: 1,
                media_type: "text/plain".to_owned(),
                retention_class: "canonical".to_owned(),
                retain_until: None,
                asserted_sensitivity: kernel::Sensitivity::Normal,
                provider_egress: kernel::ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap();
        Descriptor {
            evidence_id: handle.evidence_id,
            text: text.to_owned(),
        }
    }

    fn score(&self, descriptors: &[Descriptor]) -> CandidateCount {
        let budget = kernel::applicability::EvalBudget::new(
            Some(Instant::now() + self.counts.bounds().budget),
            Arc::new(AtomicBool::new(false)),
        );
        let tip = self.kernel.tip().unwrap();
        let jobs = self.store.memory_reviewer_status_facts().unwrap();
        let end = score_publication(
            &self.kernel,
            &self.store,
            &self.project,
            descriptors,
            &self.counts,
            &budget,
            now_ms(),
        );
        assert_eq!(self.kernel.tip().unwrap(), tip, "a pass commits nothing");
        assert_eq!(self.store.memory_reviewer_status_facts().unwrap(), jobs);
        end
    }

    fn count(&self, name: &str) -> u64 {
        self.counts.report()[name].as_u64().unwrap()
    }

    fn rows_for(&self, evidence_id: &str) -> Vec<String> {
        rows(&self.store)
            .into_iter()
            .filter(|row| row.evidence_id == evidence_id)
            .map(|row| row.decision_object_id)
            .collect()
    }
}

#[test]
fn the_candidate_set_keeps_the_newest_eligible_decisions_of_the_project_and_counts_the_cut() {
    let core = Core::open(PassBounds {
        decisions: 4,
        ..PassBounds::default()
    });
    for decision in [
        ("oldest", TOPIC, EventKind::Other),
        ("stale", TOPIC, EventKind::MarkStale),
        ("disputed", TOPIC, EventKind::MarkDisputed),
        ("contradicted", TOPIC, EventKind::Contradict),
        ("active", TOPIC, EventKind::Other),
        ("newest", TOPIC, EventKind::Other),
    ] {
        seed(&core.kernel, &core.project, &[decision]);
    }
    let other = kernel_routes::ProjectBinding::new(&core.dir.path().join("other"));
    seed(
        &core.kernel,
        &other,
        &[("foreign", TOPIC, EventKind::Other)],
    );
    let descriptor = core.descriptor("d0", TOPIC);
    assert_eq!(
        core.score(std::slice::from_ref(&descriptor)),
        CandidateCount::Completed
    );
    assert_eq!(core.count("candidates_truncated"), 1);
    let mut kept = core.rows_for(&descriptor.evidence_id);
    kept.sort();
    assert_eq!(
        kept,
        [
            "object-active",
            "object-disputed",
            "object-newest",
            "object-stale"
        ],
        "explicit search hides the contradicted one, the oldest is cut, and the other project's is never read"
    );
    assert_eq!(core.count("summaries_loaded"), 4);
}

#[test]
fn a_superseded_decision_is_never_scored_and_the_default_bounds_are_the_named_constants() {
    use crate::contradiction_candidates::pass::{
        CANDIDATE_PASS_BUDGET, CANDIDATES_PER_PASS, MAX_CANDIDATE_DECISIONS,
        MAX_CANDIDATE_ROWS_PER_PROJECT, MAX_CANDIDATE_SUMMARY_BYTES,
    };
    assert_eq!(
        PassBounds::default(),
        PassBounds {
            rows_per_pass: CANDIDATES_PER_PASS,
            decisions: MAX_CANDIDATE_DECISIONS,
            summary_bytes: MAX_CANDIDATE_SUMMARY_BYTES,
            rows_per_project: MAX_CANDIDATE_ROWS_PER_PROJECT,
            budget: CANDIDATE_PASS_BUDGET,
        }
    );
    assert_eq!(
        (
            CANDIDATES_PER_PASS,
            MAX_CANDIDATE_DECISIONS,
            MAX_CANDIDATE_ROWS_PER_PROJECT
        ),
        (64, 4096, 4096)
    );
    let core = Core::open(PassBounds::default());
    seed(
        &core.kernel,
        &core.project,
        &[("replaced", TOPIC, EventKind::Other)],
    );
    let successor = DecisionSpec {
        decision_id: "decision-replacement".to_owned(),
        object_id: "object-replacement".to_owned(),
        domain_id: crate::canonical_memory::MEMORY_DOMAIN_ID.to_owned(),
        proposition_id: None,
        scope_id: Some(core.project.scope_id()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_owned(),
        payload: DecisionPayload {
            summary: "unrelated wording about another subject entirely".to_owned(),
            rationale: "fixture".to_owned(),
        },
        source_kind: "fixture".to_owned(),
        source_id: "lineage-replaced".to_owned(),
        source_revision: 2,
        sensitivity: kernel::Sensitivity::Normal,
    };
    core.kernel
        .commit(intent("supersede"), |envelope| {
            envelope.correct_decision("object-replaced", successor)?;
            Ok(String::new())
        })
        .unwrap();
    let descriptor = core.descriptor("d0", TOPIC);
    core.score(std::slice::from_ref(&descriptor));
    assert!(core.rows_for(&descriptor.evidence_id).is_empty());
    assert_eq!(
        core.count("summaries_loaded"),
        0,
        "the replaced decision is no longer served, and its replacement has no admission of its own"
    );
}

#[test]
fn the_summary_byte_cut_skips_payloads_before_any_load() {
    let core = Core::open(PassBounds::default());
    seed(
        &core.kernel,
        &core.project,
        &[
            ("first", TOPIC, EventKind::Other),
            ("second", TOPIC, EventKind::Other),
            ("third", TOPIC, EventKind::Other),
        ],
    );
    let (_, size) = core
        .kernel
        .decision_payload_sizes_as_of(&["object-first".to_owned()], core.kernel.tip().unwrap())
        .unwrap()[0];
    *core.counts.test_bounds.lock().unwrap() = Some(PassBounds {
        summary_bytes: size + size / 2,
        ..PassBounds::default()
    });
    let descriptor = core.descriptor("d0", TOPIC);
    core.score(std::slice::from_ref(&descriptor));
    assert_eq!(core.count("summaries_loaded"), 1);
    assert_eq!(core.count("summaries_cut_bytes"), 2 * size);
    assert!(core.count("summaries_cut_bytes") > 0);
    assert_eq!(core.rows_for(&descriptor.evidence_id).len(), 1);
}

#[test]
fn an_oversize_descriptor_writes_nothing_and_an_unscorable_summary_leaves_the_others() {
    let core = Core::open(PassBounds::default());
    let huge = format!("{TOPIC} {}", "word ".repeat(2_000));
    seed(
        &core.kernel,
        &core.project,
        &[
            ("plain", TOPIC, EventKind::Other),
            ("huge", &huge, EventKind::Other),
            ("stopwords", "the and with from this", EventKind::Other),
        ],
    );
    let oversize = Descriptor {
        evidence_id: "srcev:oversize".to_owned(),
        text: "x".repeat(65_537),
    };
    let scored = core.descriptor("d1", TOPIC);
    assert_eq!(
        core.score(&[oversize.clone(), scored.clone()]),
        CandidateCount::Completed
    );
    assert_eq!(core.count("descriptors_refused_oversize"), 1);
    assert!(core.rows_for(&oversize.evidence_id).is_empty());
    assert_eq!(
        core.count("summaries_unscorable"),
        2,
        "the over-bound summary and the one the floor empties"
    );
    assert_eq!(core.rows_for(&scored.evidence_id), ["object-plain"]);
}

#[test]
fn a_deadline_during_the_third_descriptor_leaves_the_first_two() {
    let core = Core::open(PassBounds {
        budget: Duration::from_millis(500),
        ..PassBounds::default()
    });
    let names: Vec<String> = (0..5).map(|index| format!("c{index}")).collect();
    let decisions: Vec<(&str, &str, EventKind)> = names
        .iter()
        .map(|name| (name.as_str(), TOPIC, EventKind::Other))
        .collect();
    seed(&core.kernel, &core.project, &decisions);
    let descriptors: Vec<Descriptor> = (0..5)
        .map(|index| core.descriptor(&format!("d{index}"), &format!("{TOPIC} {index}")))
        .collect();
    *core.counts.before_descriptor.lock().unwrap() = Some(Arc::new(|index| {
        if index == 2 {
            std::thread::sleep(Duration::from_millis(700));
        }
    }));
    assert_eq!(core.score(&descriptors), CandidateCount::Deadline);
    for (index, descriptor) in descriptors.iter().enumerate() {
        assert_eq!(
            core.rows_for(&descriptor.evidence_id).len(),
            if index < 2 { 4 } else { 0 },
            "descriptor {index}"
        );
    }
    assert_eq!(core.count("descriptors_unscored"), 3);
    assert_eq!(core.count("deadline"), 1);
}

#[test]
fn a_budget_spent_before_the_pass_writes_nothing_and_counts_every_descriptor() {
    let core = Core::open(PassBounds {
        budget: Duration::ZERO,
        ..PassBounds::default()
    });
    seed(
        &core.kernel,
        &core.project,
        &[("one", TOPIC, EventKind::Other)],
    );
    let descriptors: Vec<Descriptor> = (0..3)
        .map(|index| core.descriptor(&format!("d{index}"), TOPIC))
        .collect();
    assert_eq!(core.score(&descriptors), CandidateCount::Deadline);
    assert!(rows(&core.store).is_empty());
    assert_eq!(core.count("descriptors_unscored"), 3);
    assert_eq!(core.count("deadline"), 1);
}

#[test]
fn the_row_caps_hold_per_descriptor_per_pass_and_per_project_and_a_replay_keeps_the_rows() {
    let names: Vec<String> = (0..5).map(|index| format!("c{index}")).collect();
    let decisions: Vec<(&str, &str, EventKind)> = names
        .iter()
        .map(|name| (name.as_str(), TOPIC, EventKind::Other))
        .collect();
    let core = Core::open(PassBounds {
        rows_per_pass: 6,
        ..PassBounds::default()
    });
    seed(&core.kernel, &core.project, &decisions);
    let first = core.descriptor("d0", TOPIC);
    let second = core.descriptor("d1", &format!("{TOPIC} again"));
    core.score(&[first.clone(), second.clone()]);
    assert_eq!(
        core.rows_for(&first.evidence_id).len(),
        4,
        "K + 1 overlaps keep K"
    );
    assert_eq!(
        core.rows_for(&second.evidence_id).len(),
        2,
        "M cuts in publish order"
    );
    assert_eq!(core.count("rows_written"), 6);
    assert_eq!(core.count("dropped_at_cap"), 2);
    let before = rows(&core.store);
    seed(
        &core.kernel,
        &core.project,
        &[(
            "later",
            "unrelated wording elsewhere entirely",
            EventKind::Other,
        )],
    );
    core.score(&[first.clone(), second.clone()]);
    let after = rows(&core.store);
    assert_eq!(after.len(), before.len());
    for (earlier, later) in before.iter().zip(&after) {
        assert_eq!(
            (&earlier.evidence_id, &earlier.decision_object_id),
            (&later.evidence_id, &later.decision_object_id)
        );
        assert!(later.publish_tip > earlier.publish_tip);
    }
    let full = core.descriptor("d2", &format!("{TOPIC} third"));
    *core.counts.test_bounds.lock().unwrap() = Some(PassBounds {
        rows_per_pass: 4,
        ..PassBounds::default()
    });
    core.score(&[full.clone(), first.clone()]);
    assert_eq!(core.rows_for(&full.evidence_id).len(), 4);
    assert_eq!(
        core.rows_for(&first.evidence_id).len(),
        4,
        "a descriptor past the pass cap keeps the rows it already holds"
    );

    let capped = Core::open(PassBounds {
        rows_per_project: 3,
        ..PassBounds::default()
    });
    seed(&capped.kernel, &capped.project, &decisions);
    let descriptor = capped.descriptor("d0", TOPIC);
    capped.score(std::slice::from_ref(&descriptor));
    assert_eq!(rows(&capped.store).len(), 3);
    assert_eq!(capped.count("dropped_at_cap"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rerun_from_the_retained_bytes_at_one_tip_rebuilds_the_rows_and_the_kernel_is_untouched()
{
    let (handler, store, _dir, project) = candidate_handler(candidate_config(true, true)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    let kernel = kernel_of(&handler);
    seed(
        &kernel,
        &binding,
        &[
            ("one", TOPIC, EventKind::Other),
            (
                "two",
                "snapshot ledger quorum lease renewal",
                EventKind::Other,
            ),
        ],
    );
    let mut native = window(0, &format!("{TOPIC} first"));
    native.extend(window(1, "snapshot ledger quorum lease renewal second"));
    let (reached, release) = park(&handler);
    publish(&handler, &project, &native, true);
    entered(&reached);
    let tip = kernel.tip().unwrap();
    let jobs = store.memory_reviewer_status_facts().unwrap();
    release.send(()).unwrap();
    settle(&handler).await;
    unhook(&handler);
    assert_eq!(kernel.tip().unwrap(), tip, "the pass commits nothing");
    assert_eq!(store.memory_reviewer_status_facts().unwrap(), jobs);
    let written = rows(&store);
    assert!(written.len() >= 2, "{written:?}");
    assert!(written.iter().all(|row| row.publish_tip == tip));

    store.truncate_contradiction_candidates_for_test().unwrap();
    let descriptors: Vec<Descriptor> = live_messages(&kernel)
        .iter()
        .map(|row| Descriptor {
            evidence_id: row.detail.evidence_id.clone(),
            text: retained_text(&kernel, row),
        })
        .collect();
    let mut retained: Vec<&str> = descriptors.iter().map(|row| row.text.as_str()).collect();
    retained.sort_unstable();
    let first_text = format!("{TOPIC} first");
    let mut published = vec![
        first_text.as_str(),
        "snapshot ledger quorum lease renewal second",
    ];
    published.sort_unstable();
    assert_eq!(
        retained, published,
        "the Kernel retained exactly the texts the pass scored"
    );
    let counts = Candidates::default();
    let budget = kernel::applicability::EvalBudget::unbounded();
    assert_eq!(
        score_publication(
            &kernel,
            &store,
            &binding,
            &descriptors,
            &counts,
            &budget,
            now_ms()
        ),
        CandidateCount::Completed
    );
    let strip = |rows: Vec<StoredCandidate>| -> Vec<StoredCandidate> {
        rows.into_iter()
            .map(|row| StoredCandidate {
                created_at_ms: 0,
                ..row
            })
            .collect()
    };
    assert_eq!(
        strip(rows(&store)),
        strip(written),
        "the rows scored from the retained bytes equal the rows the pass wrote"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_route_consent_reaches_the_pass_through_a_transform() {
    for consent in [false, true] {
        let (handler, store, _dir, project) = handler_with_store_and_kernel(
            Arc::new(ProducerState::default()),
            candidate_config(true, !consent),
        )
        .await;
        let mut bound = binding_with_harness(project.to_str().unwrap(), "opencode", SESSION);
        bound.config = candidate_config(true, consent);
        handler.bind_route(test_route(7), bound);
        let binding = kernel_routes::ProjectBinding::new(&project);
        seed(
            &kernel_of(&handler),
            &binding,
            &[("one", TOPIC, EventKind::Other)],
        );
        let native = window(0, &format!("{TOPIC} from a transform"));
        let messages: Vec<IngressMessage> = codec::decode_opencode(&native)
            .messages
            .into_iter()
            .map(|mut message| {
                message.ck.meta.created_at_ms = Some(1_000 + message.ordinal as i64);
                message
            })
            .collect();
        let mut request = request_with_usage(messages, 1_000, 200_000);
        request["serializer_profile"] = json!("opencode-aisdk");
        request["native_messages"] = json!(native);
        request["base_revision"] = json!("base-0");
        let request = handler.test_client_prepare_request(request);
        let mut outcome = handler
            .handle_transform_for_test(test_route(7), request)
            .await;
        outcome
            .take_after_response()
            .expect("capture runs after the response")
            .run();
        settle(&handler).await;
        assert_eq!(capture_report(&handler)["units_published"], 1);
        assert_eq!(
            counts(&handler)["offered"],
            u64::from(consent),
            "consent {consent}"
        );
        assert_eq!(counts(&handler)["completed"], u64::from(consent));
        assert_eq!(rows(&store).len(), usize::from(consent));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_pass_releases_its_permit_and_charge_and_shutdown_waits_for_a_parked_pass() {
    let (handler, store, _dir, project) = candidate_handler(candidate_config(true, true)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    seed(
        &kernel_of(&handler),
        &binding,
        &[("one", TOPIC, EventKind::Other)],
    );
    *handler.contradiction_candidates.before_pass.lock().unwrap() =
        Some(Arc::new(|| panic!("injected pass panic")));
    publish(
        &handler,
        &project,
        &window(0, &format!("{TOPIC} first")),
        true,
    );
    until(&handler, "worker_panicked", 1).await;
    settle(&handler).await;
    unhook(&handler);
    assert_eq!(counts(&handler)["active"], 0);
    assert_eq!(capture_report(&handler)["retained_bytes"], 0);
    assert!(rows(&store).is_empty());

    let job_charge = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&job_charge);
    let capture = Arc::clone(&handler.source_capture);
    *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
        let held = capture.report(false, now_ms())["retained_bytes"]
            .as_u64()
            .unwrap();
        observed.store(held, std::sync::atomic::Ordering::Relaxed);
    }));
    let (reached, release) = park(&handler);
    let text = format!("{TOPIC} second");
    publish(&handler, &project, &window(1, &text), true);
    entered(&reached);
    *handler.source_capture.before_publish.lock().unwrap() = None;
    let held = capture_report(&handler)["retained_bytes"].as_u64().unwrap();
    let job = job_charge.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        held > text.len() as u64 && held < job,
        "the parked pass holds a charge for its kept text ({held} bytes), below the job's {job}"
    );
    let core = Arc::clone(&handler.core);
    let shutdown =
        tokio::spawn(async move { CompositeComponent::shutdown(&Handler { core }).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !shutdown.is_finished(),
        "shutdown waits for the parked pass"
    );
    release.send(()).unwrap();
    shutdown.await.unwrap().unwrap();
    assert_eq!(counts(&handler)["active"], 0);
    assert_eq!(capture_report(&handler)["retained_bytes"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publish_latency_with_candidates_on_stays_within_the_off_baseline() {
    const RUNS: u64 = 12;
    let mut medians = Vec::new();
    for enabled in [false, true] {
        let (handler, _store, _dir, project) =
            candidate_handler(candidate_config(true, enabled)).await;
        let binding = kernel_routes::ProjectBinding::new(&project);
        let decisions: Vec<(String, String)> = (0..2_000)
            .map(|index| (format!("seeded-{index}"), format!("{TOPIC} {index} topic")))
            .collect();
        let refs: Vec<(&str, &str, EventKind)> = decisions
            .iter()
            .map(|(name, summary)| (name.as_str(), summary.as_str(), EventKind::Other))
            .collect();
        seed(&kernel_of(&handler), &binding, &refs);
        // The flag-on arm records each publication that commits while a pass is running.
        let overlapped = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&overlapped);
        let candidates = Arc::clone(&handler.contradiction_candidates);
        *handler.source_capture.before_publish.lock().unwrap() = Some(Arc::new(move || {
            if candidates.report()["active"] == 1 {
                seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }));
        let mut samples = Vec::new();
        for run in 0..RUNS {
            let batch = |offset: u64, word: &str| -> Vec<Value> {
                (0..8)
                    .flat_map(|unit| {
                        window(
                            offset + run * 8 + unit,
                            &format!("{TOPIC} {run} {unit} {word}"),
                        )
                    })
                    .collect()
            };
            let parked = enabled.then(|| park(&handler));
            publish(&handler, &project, &batch(0, "topic"), enabled);
            if let Some((reached, release)) = &parked {
                entered(reached);
                unhook(&handler);
                release.send(()).unwrap();
            } else {
                while !handler.source_capture.is_idle() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }
            samples.push(
                capture_report(&handler)["publish_ms_last"]
                    .as_u64()
                    .unwrap(),
            );
            publish(&handler, &project, &batch(10_000, "beside"), enabled);
            while !handler.source_capture.is_idle() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            samples.push(
                capture_report(&handler)["publish_ms_last"]
                    .as_u64()
                    .unwrap(),
            );
            settle(&handler).await;
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        let overlaps = overlapped.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!(
            "candidate pass {}: publication hold median {median} ms, p90 {} ms, max {} ms over {} publications; pass_ms_max {}; publications beside a running pass {overlaps}",
            if enabled { "on" } else { "off" },
            samples[samples.len() * 9 / 10],
            samples[samples.len() - 1],
            samples.len(),
            counts(&handler)["pass_ms_max"],
        );
        medians.push(median);
        if enabled {
            assert!(overlaps > 0, "no publication committed while a pass ran");
        }
    }
    assert!(
        medians[1] <= medians[0].saturating_mul(3) + 50,
        "median publication hold with candidates on {} ms, off {} ms",
        medians[1],
        medians[0]
    );
}

fn activate_memories_authority(store: &MemoryStore, root: &Path) {
    let project = "git:candidates";
    let preparing = store
        .authority_begin_prepare("ctx", project, "memories")
        .unwrap();
    let checksum = store
        .authority_seed_checksum("ctx", project, "memories")
        .unwrap();
    store
        .authority_verify_prepare(
            "ctx",
            project,
            "memories",
            preparing.generation,
            &checksum,
            &checksum,
        )
        .unwrap();
    store
        .authority_ack_prepare("ctx", project, "memories", preparing.generation)
        .unwrap();
    store
        .bind_authority_route("ctx", project, root.to_str().unwrap())
        .unwrap();
}

fn candidates_request(project: &Path, body: Value) -> Value {
    let mut request = json!({
        "method": "review.candidates",
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
    });
    for (key, value) in body.as_object().unwrap() {
        request[key] = value.clone();
    }
    request
}

async fn route_call(handler: &Handler, request: Value) -> Result<Value, String> {
    match handler
        .dispatch_value_for_test(test_route(7), request)
        .await
    {
        PreparedOutcome::Response(output) => {
            let mut bytes = Vec::new();
            output.measure().unwrap().write_to(&mut bytes).unwrap();
            Ok(serde_json::from_slice(&bytes).unwrap())
        }
        PreparedOutcome::Error { code, .. } => Err(code),
        PreparedOutcome::Streamed => panic!("the route streamed"),
    }
}

fn ingest(kernel: &KernelStore, key: &str) -> String {
    let payload = format!("descriptor {key}").into_bytes();
    let mut request = intent(&format!("evidence-{key}"));
    request.request_digest = kernel::source_identity::identity_digest(&payload);
    kernel
        .ingest_exact_artifact(kernel::ArtifactIngestRequest {
            intent: request,
            payload,
            evidence_id: format!("srcev:{key}"),
            object_id: format!("srcev-object:{key}"),
            object_kind: "evidence".to_owned(),
            domain_id: crate::canonical_memory::MEMORY_DOMAIN_ID.to_owned(),
            source_kind: "messages".to_owned(),
            source_id: format!("native/{key}"),
            source_revision: 1,
            media_type: "text/plain".to_owned(),
            retention_class: "canonical".to_owned(),
            retain_until: None,
            asserted_sensitivity: kernel::Sensitivity::Normal,
            provider_egress: kernel::ProviderEgress::LocalOnly,
            provenance: None,
        })
        .unwrap()
        .evidence_id
}

fn write_row(
    store: &MemoryStore,
    project: &kernel_routes::ProjectBinding,
    evidence: &str,
    decision: &str,
    matched: u32,
    (incarnation, digest, tip): (&str, &str, i64),
) {
    let object = format!("object-{decision}");
    let id = format!("decision-{decision}");
    store
        .replace_candidates_for_descriptor(
            &memory_store::contradiction_candidates::DescriptorCandidates {
                project_digest: project.digest(),
                evidence_id: evidence,
                publish_tip: tip,
                kernel_incarnation_id: incarnation,
                matcher_digest: digest,
                created_at_ms: now_ms(),
                rows: &[memory_store::contradiction_candidates::CandidateScore {
                    decision_object_id: &object,
                    decision_id: &id,
                    matched_terms: matched,
                    decision_terms: 5,
                }],
            },
            memory_store::contradiction_candidates::CandidateCaps {
                per_descriptor: 4,
                per_project: 4096,
            },
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_candidates_pages_live_rows_and_answers_each_terminal() {
    let (handler, store, dir, project) = candidate_handler(candidate_config(true, true)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    let kernel = kernel_of(&handler);
    seed(
        &kernel,
        &binding,
        &[
            ("one", TOPIC, EventKind::Other),
            ("replaced", TOPIC, EventKind::Other),
        ],
    );
    let ask = |body: Value| route_call(&handler, candidates_request(&project, body));
    assert_eq!(
        ask(json!({ "decision_object_id": "object-one" })).await,
        Ok(json!({ "kind": "terminal", "terminal": "disabled" })),
        "a root without MODULE memories authority"
    );
    activate_memories_authority(&store, &project);
    let incarnation = kernel
        .capture_identified_commit_read_target_within_budget(
            &kernel::applicability::EvalBudget::unbounded(),
        )
        .unwrap()
        .database_incarnation_id;
    let digest = crate::contradiction_candidates::scorer::matcher_digest();
    let evidence: Vec<String> = (0..70)
        .map(|index| ingest(&kernel, &format!("e{index}")))
        .collect();
    let retired = ingest(&kernel, "retired");
    let tip = kernel.tip().unwrap();
    for (index, evidence) in evidence.iter().enumerate() {
        write_row(
            &store,
            &binding,
            evidence,
            "one",
            3 + u32::from(index < 35),
            (&incarnation, digest, tip),
        );
    }
    write_row(
        &store,
        &binding,
        &retired,
        "one",
        5,
        (&incarnation, digest, tip),
    );
    let replaced_evidence = ingest(&kernel, "replaced");
    write_row(
        &store,
        &binding,
        &replaced_evidence,
        "replaced",
        5,
        (&incarnation, digest, tip),
    );
    let foreign = ingest(&kernel, "foreign");
    let stale = ingest(&kernel, "stale");
    let tip = kernel.tip().unwrap();
    write_row(
        &store,
        &binding,
        &foreign,
        "one",
        3,
        (&"0".repeat(32), digest, tip),
    );
    write_row(
        &store,
        &binding,
        &stale,
        "one",
        3,
        (&incarnation, &"f".repeat(64), tip),
    );
    let future = ingest(&kernel, "future");
    kernel
        .commit(intent("retire-evidence"), |envelope| {
            envelope.retire_evidence("srcev-object:retired")?;
            Ok(String::new())
        })
        .unwrap();
    let tip = kernel.tip().unwrap();
    write_row(
        &store,
        &binding,
        &future,
        "one",
        3,
        (&incarnation, digest, tip + 1),
    );

    let jobs = store.memory_reviewer_status_facts().unwrap();
    let written = rows(&store);
    let first = ask(json!({ "decision_object_id": "object-one", "limit": 65 }))
        .await
        .unwrap();
    assert_eq!(first["kind"], "page");
    assert_eq!(first["as_of"], tip);
    assert_eq!(kernel.tip().unwrap(), tip, "the read commits nothing");
    assert_eq!(store.memory_reviewer_status_facts().unwrap(), jobs);
    let items = first["items"].as_array().unwrap();
    let keys: Vec<&str> = items[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut keys = keys;
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "created_at_ms",
            "decision_id",
            "decision_object_id",
            "decision_terms",
            "evidence_id",
            "matched_terms",
            "publish_tip",
            "score"
        ]
    );
    let first_filtered = counts(&handler)["read_filtered_stale"].as_u64().unwrap();
    assert_eq!(
        items.len() as u64 + first_filtered,
        64,
        "a limit of 65 clamps to 64 rows, each listed or filtered"
    );
    let scores: Vec<&Value> = items.iter().map(|item| &item["score"]).collect();
    assert!(
        items[..35].iter().all(|item| item["score"] == 8000),
        "{scores:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item["evidence_id"] != json!(retired))
    );
    assert!(first["next"].is_string());
    let second = ask(json!({
        "decision_object_id": "object-one",
        "limit": 64,
        "after": first["next"],
    }))
    .await
    .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 70 - items.len());
    assert_eq!(second["next"], Value::Null);
    assert_eq!(
        counts(&handler)["read_filtered_stale"],
        4,
        "the retired, foreign-incarnation, stale-digest, and future-tip rows"
    );
    assert_eq!(rows(&store), written, "the read deletes no stale row");
    let walked: Vec<&Value> = items
        .iter()
        .chain(second["items"].as_array().unwrap())
        .collect();
    let mut ordered = walked.clone();
    ordered.sort_by(|left, right| {
        let key = |item: &Value| {
            (
                std::cmp::Reverse(item["score"].as_u64()),
                std::cmp::Reverse(item["created_at_ms"].as_u64()),
                item["evidence_id"].as_str().unwrap().to_owned(),
            )
        };
        key(left).cmp(&key(right))
    });
    assert_eq!(
        walked, ordered,
        "the walk follows the documented page order"
    );
    let listed: std::collections::BTreeSet<&str> = walked
        .iter()
        .map(|item| item["evidence_id"].as_str().unwrap())
        .collect();
    let expected: std::collections::BTreeSet<&str> = evidence.iter().map(String::as_str).collect();
    assert_eq!(
        listed, expected,
        "the pages are disjoint and cover every live row"
    );
    let filtered = counts(&handler)["read_filtered_stale"].as_u64().unwrap();
    let smallest = ask(json!({ "decision_object_id": "object-one", "limit": 0 }))
        .await
        .unwrap();
    assert!(smallest["next"].is_string());
    assert_eq!(
        smallest["items"].as_array().unwrap().len() as u64
            + counts(&handler)["read_filtered_stale"].as_u64().unwrap()
            - filtered,
        1,
        "a limit of zero clamps to one row"
    );

    kernel
        .commit(intent("supersede-replaced"), |envelope| {
            envelope.correct_decision(
                "object-replaced",
                DecisionSpec {
                    decision_id: "decision-successor".to_owned(),
                    object_id: "object-successor".to_owned(),
                    domain_id: crate::canonical_memory::MEMORY_DOMAIN_ID.to_owned(),
                    proposition_id: None,
                    scope_id: Some(binding.scope_id()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "architecture".to_owned(),
                    payload: DecisionPayload {
                        summary: "a successor".to_owned(),
                        rationale: "fixture".to_owned(),
                    },
                    source_kind: "fixture".to_owned(),
                    source_id: "lineage-replaced".to_owned(),
                    source_revision: 2,
                    sensitivity: kernel::Sensitivity::Normal,
                },
            )?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(
        ask(json!({ "decision_object_id": "object-replaced" })).await,
        Ok(json!({ "kind": "terminal", "terminal": "not_live" })),
        "rows exist for the replaced decision"
    );
    assert_eq!(
        ask(json!({ "decision_object_id": "object-missing" })).await,
        Ok(json!({ "kind": "terminal", "terminal": "not_live" }))
    );
    for body in [
        json!({ "decision_object_id": "object-one", "extra": 1 }),
        json!({ "decision_object_id": "object-one", "after": "not-a-cursor" }),
        json!({ "decision_object_id": "" }),
        json!({}),
    ] {
        assert_eq!(
            ask(body.clone()).await,
            Err("invalid_params".to_owned()),
            "{body}"
        );
    }
    let elsewhere = project.parent().unwrap().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let refused = route_call(
        &handler,
        candidates_request(&elsewhere, json!({ "decision_object_id": "object-one" })),
    )
    .await
    .unwrap();
    assert_eq!(refused["state"]["kind"], "invalid");
    assert_eq!(refused["state"]["reason"], "project_mismatch");
    let refused = route_call(
        &handler,
        candidates_request(&elsewhere, json!({ "extra": 1 })),
    )
    .await
    .unwrap();
    assert_eq!(
        refused["state"]["reason"], "project_mismatch",
        "the scope is judged before the body"
    );

    let rebind = |harness: &str, enabled: bool| {
        let mut bound = binding_with_harness(project.to_str().unwrap(), harness, SESSION);
        bound.config = candidate_config(true, enabled);
        handler.bind_route(test_route(7), bound);
    };
    rebind(crate::OBSERVATIONAL_HARNESS, true);
    let observed = ask(json!({ "decision_object_id": "object-one" }))
        .await
        .unwrap();
    assert_eq!(observed["kind"], "page", "an observational route reads");

    let path = find_file(dir.path(), "store.db");
    let rename = |from: &str, to: &str| {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}"))
            .unwrap();
    };
    rename(
        "contradiction_candidates",
        "contradiction_candidates_hidden",
    );
    rebind("opencode", false);
    let filtered = counts(&handler)["read_filtered_stale"].clone();
    assert_eq!(
        ask(json!({ "decision_object_id": "object-one" })).await,
        Ok(json!({ "kind": "terminal", "terminal": "disabled" })),
        "a false flag answers before any table statement"
    );
    assert_eq!(counts(&handler)["read_filtered_stale"], filtered);
    rebind("opencode", true);
    assert_eq!(
        ask(json!({ "decision_object_id": "object-one" })).await,
        Ok(json!({ "kind": "terminal", "terminal": "store_unavailable" })),
        "the hidden table fails the read a true flag runs"
    );
    rename(
        "contradiction_candidates_hidden",
        "contradiction_candidates",
    );
}

fn find_file(root: &Path, name: &str) -> PathBuf {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|file| file == name) {
                return path;
            }
        }
    }
    panic!("no {name} under {}", root.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_candidates_answers_the_store_and_kernel_states_before_any_page() {
    let (starting, _store, _dir, project) = handler_with_store(
        Arc::new(ProducerState::default()),
        candidate_config(true, true),
    );
    let mut bound = binding_with_harness(project.to_str().unwrap(), "opencode", SESSION);
    bound.config = candidate_config(true, true);
    starting.bind_route(test_route(7), bound);
    let answer = route_call(
        &starting,
        candidates_request(&project, json!({ "decision_object_id": "object-one" })),
    )
    .await
    .unwrap();
    assert_eq!(answer["state"]["kind"], "unavailable", "{answer}");
    assert_eq!(answer["state"]["reason"], "store_starting", "{answer}");
    assert!(answer.get("items").is_none());

    let blocked = _dir.path().join("kernel-is-a-file");
    std::fs::write(&blocked, b"").unwrap();
    starting.kernel.set_expected(true);
    starting
        .kernel
        .open(blocked, StoreOpenPolicy::default(), starting.cancel.clone())
        .await;
    let answer = route_call(
        &starting,
        candidates_request(&project, json!({ "decision_object_id": "object-one" })),
    )
    .await
    .unwrap();
    assert_eq!(answer["state"]["kind"], "unavailable", "{answer}");
    assert_ne!(answer["state"]["reason"], "store_starting", "{answer}");
    assert!(answer.get("items").is_none());

    let dir = tempfile::tempdir().unwrap();
    let storeless = Handler::with_producer_factory_config_resolver(
        Arc::new(TestProducerFactory {
            state: Arc::new(ProducerState::default()),
        }),
        candidate_config(true, true),
        Arc::new(MissingSessionResolver),
    );
    storeless.kernel.set_expected(true);
    storeless
        .kernel
        .open(
            dir.path().join("kernel"),
            StoreOpenPolicy::default(),
            storeless.cancel.clone(),
        )
        .await;
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let mut bound = binding_with_harness(root.to_str().unwrap(), "opencode", SESSION);
    bound.config = candidate_config(true, true);
    storeless.bind_route(test_route(7), bound);
    assert_eq!(
        route_call(
            &storeless,
            candidates_request(&root, json!({ "decision_object_id": "object-one" })),
        )
        .await,
        Ok(json!({ "kind": "terminal", "terminal": "store_unavailable" }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_listed_candidate_confirms_through_dispute_while_its_evidence_lives() {
    let (handler, store, _dir, project) = candidate_handler(candidate_config(true, true)).await;
    let binding = kernel_routes::ProjectBinding::new(&project);
    let kernel = kernel_of(&handler);
    activate_memories_authority(&store, &project);
    seed(
        &kernel,
        &binding,
        &[
            ("confirmed", TOPIC, EventKind::Other),
            (
                "expired",
                "harbor lighthouse ferry anchor dock",
                EventKind::Other,
            ),
        ],
    );
    publish(
        &handler,
        &project,
        &window(0, &format!("{TOPIC} first")),
        true,
    );
    settle(&handler).await;
    publish(
        &handler,
        &project,
        &window(1, "harbor lighthouse ferry anchor dock second"),
        true,
    );
    settle(&handler).await;
    let ask = |decision: &str| {
        route_call(
            &handler,
            candidates_request(&project, json!({ "decision_object_id": decision })),
        )
    };
    let listed = ask("object-confirmed").await.unwrap();
    let confirmed_row = listed["items"][0].clone();
    let expired_row = ask("object-expired").await.unwrap()["items"][0].clone();
    let published: Vec<String> = live_messages(&kernel)
        .into_iter()
        .map(|descriptor| descriptor.detail.evidence_id)
        .collect();
    assert!(
        published.contains(&confirmed_row["evidence_id"].as_str().unwrap().to_owned()),
        "{confirmed_row} cites a published descriptor of {published:?}"
    );
    let dispute = |row: &Value, key: &str| {
        crate::kernel_route_fixtures::commit_request(
            &project,
            SESSION,
            key,
            vec![json!({
                "op": "disposition",
                "object_id": row["decision_object_id"],
                "event": "mark_disputed",
                "evidence_id": row["evidence_id"],
            })],
            vec![],
        )
    };
    let kept = rows(&store)
        .into_iter()
        .find(|row| row.decision_object_id == "object-expired")
        .unwrap();
    store.truncate_contradiction_candidates_for_test().unwrap();
    let applied = route_call(&handler, dispute(&confirmed_row, "confirm-1"))
        .await
        .unwrap();
    write_row(
        &store,
        &binding,
        &kept.evidence_id,
        "expired",
        kept.matched_terms,
        (
            &kept.kernel_incarnation_id,
            &kept.matcher_digest,
            kept.publish_tip,
        ),
    );
    assert_eq!(applied["state"]["kind"], "available", "{applied}");
    assert_eq!(applied["dispositions"][0]["disposition"], "disputed");

    let later = now_ms() + 31 * 24 * 60 * 60 * 1000;
    kernel.expire_harness_captures(later).unwrap();
    let filtered = counts(&handler)["read_filtered_stale"].as_u64().unwrap();
    let after = ask("object-expired").await.unwrap();
    assert_eq!(
        after["items"],
        json!([]),
        "the retired evidence's row is left out"
    );
    assert_eq!(
        counts(&handler)["read_filtered_stale"].as_u64().unwrap(),
        filtered + 1
    );
    let refused = route_call(&handler, dispute(&expired_row, "confirm-2"))
        .await
        .unwrap();
    assert_eq!(refused["state"]["kind"], "invalid", "{refused}");
    assert_eq!(refused["state"]["reason"], "not_found");
}

#[test]
fn the_confirm_path_never_reads_the_candidate_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kernel_routes");
    for name in ["commit.rs", "dispute.rs"] {
        let text = std::fs::read_to_string(root.join(name)).unwrap();
        for token in [
            "candidates_for_decision",
            "contradiction_candidates",
            "candidate_sweep_page",
            "replace_candidates_for_descriptor",
            "delete_candidates_as_read",
            "candidate_rows_and_bytes",
        ] {
            assert!(!text.contains(token), "{name} names {token}");
        }
    }
}
