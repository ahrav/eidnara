use std::num::NonZeroUsize;
use std::sync::atomic::AtomicBool;
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
        let (handler, store, _dir, project) =
            candidate_handler(candidate_config(true, consent)).await;
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

    let (reached, release) = park(&handler);
    publish(
        &handler,
        &project,
        &window(1, &format!("{TOPIC} second")),
        true,
    );
    entered(&reached);
    let held = capture_report(&handler)["retained_bytes"].as_u64().unwrap();
    assert!(held > 0, "the parked pass holds its publication's charge");
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
    let mut held = Vec::new();
    for enabled in [false, true] {
        let (handler, _store, _dir, project) =
            candidate_handler(candidate_config(true, enabled)).await;
        let binding = kernel_routes::ProjectBinding::new(&project);
        let decisions: Vec<(String, String)> = (0..64)
            .map(|index| (format!("seeded-{index}"), format!("{TOPIC} {index} topic")))
            .collect();
        let refs: Vec<(&str, &str, EventKind)> = decisions
            .iter()
            .map(|(name, summary)| (name.as_str(), summary.as_str(), EventKind::Other))
            .collect();
        seed(&kernel_of(&handler), &binding, &refs);
        for run in 0..RUNS {
            let texts: Vec<Value> = (0..8)
                .flat_map(|unit| window(run * 8 + unit, &format!("{TOPIC} {run} {unit} topic")))
                .collect();
            // On the flag-on arm every odd publication runs while the previous pass is parked.
            let parked = (enabled && run % 2 == 0).then(|| park(&handler));
            publish(&handler, &project, &texts, enabled);
            if let Some((reached, release)) = parked {
                entered(&reached);
                unhook(&handler);
                let next: Vec<Value> = (0..8)
                    .flat_map(|unit| {
                        window(
                            10_000 + run * 8 + unit,
                            &format!("{TOPIC} {run} {unit} overlap"),
                        )
                    })
                    .collect();
                publish(&handler, &project, &next, enabled);
                while !handler.source_capture.is_idle() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                release.send(()).unwrap();
            }
            settle(&handler).await;
        }
        let report = capture_report(&handler);
        eprintln!(
            "candidate pass {}: publish_ms_max {} publish_ms_last {} units_published {} pass_ms_max {} rows_written {} refused_pass_busy {}",
            if enabled { "on" } else { "off" },
            report["publish_ms_max"],
            report["publish_ms_last"],
            report["units_published"],
            counts(&handler)["pass_ms_max"],
            counts(&handler)["rows_written"],
            counts(&handler)["refused_pass_busy"],
        );
        held.push(report["publish_ms_max"].as_u64().unwrap());
        if enabled {
            assert_eq!(counts(&handler)["refused_pass_busy"], RUNS / 2);
            assert_eq!(counts(&handler)["completed"], RUNS);
        }
    }
    assert!(
        held[1] <= held[0].saturating_mul(2) + 50,
        "publication hold with candidates on {} ms, off {} ms",
        held[1],
        held[0]
    );
}
