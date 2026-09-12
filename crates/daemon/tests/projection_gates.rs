//! The shared projection-hook gate against real entry paths: every hook the product has reaches the gate's ledger before it does anything, a closed gate leaves every store untouched, each invalid evidence dimension denies on its own, valid evidence reaches a real slice and its expected effect, and closing the gate mid-slice cancels the slice while the admitted native call keeps its owner.

mod support;

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use daemon::embedding_dispatch::Blocked;
use daemon::embedding_supervisor::{
    EmbeddingSupervisor, Maintained, SliceBounds, SliceKind, SliceOutcome, SupervisorEvent,
};
use daemon::git_reconcile::{
    GitReconciler, InventoryBounds, ReconcileBlocked, ReconcileEnd, ReconcileScope,
};
use daemon::git_sources::{GitReadBounds, GitRefusal, RepositoryBinding, read_selection};
use daemon::message_cleanup::{CleanupBounds, CleanupStop, MessageCleanup};
use daemon::projection_gates::{
    APPROVED_OBSERVERS, CAPABILITIES, CapabilityDisposition, CapabilityEvidence, Denial,
    EntryPoint, EvidenceEvaluator, Gate, HARNESSES, HarnessRun, HookGate, InvalidationIdentity,
    ManifestRefusal, ProjectionHook, REQUIRED_LIMITS, RuntimeManifest,
};
use host_runtime::synapse::SynapseLimits;
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, KernelStore, ProjectScope};
use rusqlite::Connection;
use serde_json::{Value, json};
use support::embedding_fixtures::{
    Corpus, GateGuard, NOW, PROJECT, TestEngine, bounds, component, inspect, occurrence_of,
};
use support::projection_gate::{identity, open_gate, passing_evaluator};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn unbounded() -> EvalBudget {
    EvalBudget::new(
        Some(std::time::Instant::now() + Duration::from_secs(30)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

fn manifest_json(hooks: &[ProjectionHook]) -> Value {
    let limits: serde_json::Map<String, Value> = REQUIRED_LIMITS
        .into_iter()
        .map(|name| (name.to_owned(), json!(1_000_000)))
        .collect();
    let hooks: serde_json::Map<String, Value> = hooks
        .iter()
        .map(|hook| (hook.id().to_owned(), json!({ "enabled": true })))
        .collect();
    json!({
        "protocol_version": "limits.v1",
        "invalidation_identity": {
            "schema_version": retrieval::SCHEMA_VERSION,
            "tokenizer_fingerprint": support::projection_gate::FINGERPRINT,
            "embedding_model": support::projection_gate::MODEL,
            "projection_policy_version": support::projection_gate::POLICY,
            "identity_contract_version": support::projection_gate::CONTRACT,
            "limit_manifest_protocol_version": "limits.v1",
            "vector_dimension": 8,
            "generation_epoch": 1,
        },
        "limits": limits,
        "hooks": hooks,
    })
}

fn passing() -> EvidenceEvaluator {
    passing_evaluator(&identity("k", 8), 10, &ProjectionHook::ALL)
}

/// The durable rows of every projection table a hook may write, so "no new durable work" is a comparison rather than a claim.
fn durable_work(conn: &Connection) -> Vec<(String, i64)> {
    [
        "embedding_jobs",
        "occurrence_vectors",
        "occurrences",
        "occurrence_tombstones",
        "payloads",
    ]
    .into_iter()
    .map(|table| {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        (table.to_owned(), count)
    })
    .collect()
}

fn slice_bounds(slice: Duration) -> SliceBounds {
    SliceBounds {
        dispatch: bounds(),
        sweep_candidates: NonZeroUsize::new(16).unwrap(),
        slice,
        idle: Duration::from_millis(20),
    }
}

async fn ended(events: &mut UnboundedReceiver<SupervisorEvent>, kind: SliceKind) -> SliceOutcome {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            SupervisorEvent::SliceEnded { kind: k, outcome } if k == kind => return outcome,
            SupervisorEvent::Stopped(stop) => panic!("{stop:?}"),
            _ => {}
        }
    }
}

/// AC6: the validator accepts exactly the runtime schema and refuses each malformed manifest by name; an enabled hook under a manifest whose identity is not the projection's, and a limit the observation exceeds, are denied by the evaluator.
#[test]
fn manifest_validator_refuses_each_malformed_manifest_and_identity_mismatch() {
    let valid = manifest_json(&[ProjectionHook::MessageCleanup]);
    let parsed = RuntimeManifest::parse(&valid).unwrap();
    assert_eq!(
        parsed.limits.get("catchup_lag_commits").copied(),
        Some(1_000_000)
    );
    assert_eq!(
        parsed.enabled.get(&ProjectionHook::MessageCleanup),
        Some(&true)
    );
    assert_eq!(
        parsed.identity,
        InvalidationIdentity::from(&identity("any", 8))
    );

    let mut missing = valid.clone();
    missing["limits"]
        .as_object_mut()
        .unwrap()
        .remove("pending_bytes");
    assert_eq!(
        RuntimeManifest::parse(&missing),
        Err(ManifestRefusal::MissingLimit("pending_bytes".to_owned()))
    );
    let mut non_numeric = valid.clone();
    non_numeric["limits"]["pending_bytes"] = json!("many");
    assert_eq!(
        RuntimeManifest::parse(&non_numeric),
        Err(ManifestRefusal::NonNumericLimit("pending_bytes".to_owned()))
    );
    let mut negative = valid.clone();
    negative["limits"]["pending_bytes"] = json!(-1);
    assert_eq!(
        RuntimeManifest::parse(&negative),
        Err(ManifestRefusal::NonNumericLimit("pending_bytes".to_owned()))
    );
    let mut extra_limit = valid.clone();
    extra_limit["limits"]["approval_digest"] = json!(1);
    assert_eq!(
        RuntimeManifest::parse(&extra_limit),
        Err(ManifestRefusal::UnknownLimit("approval_digest".to_owned()))
    );
    let mut no_protocol = valid.clone();
    no_protocol["protocol_version"] = json!("");
    assert_eq!(
        RuntimeManifest::parse(&no_protocol),
        Err(ManifestRefusal::MissingProtocolVersion)
    );
    assert_eq!(
        RuntimeManifest::parse(&json!([])),
        Err(ManifestRefusal::Shape)
    );
    let mut provenance = valid.clone();
    provenance["approvers"] = json!(["alice"]);
    assert_eq!(
        RuntimeManifest::parse(&provenance),
        Err(ManifestRefusal::Shape),
        "campaign provenance is not part of the runtime schema"
    );
    let mut unknown_hook = valid.clone();
    unknown_hook["hooks"]["search_projection.mural"] = json!({ "enabled": true });
    assert_eq!(
        RuntimeManifest::parse(&unknown_hook),
        Err(ManifestRefusal::UnknownHook(
            "search_projection.mural".to_owned()
        ))
    );
    let mut malformed_flag = valid.clone();
    malformed_flag["hooks"][ProjectionHook::MessageCleanup.id()] = json!({ "enabled": "yes" });
    assert_eq!(
        RuntimeManifest::parse(&malformed_flag),
        Err(ManifestRefusal::MalformedFlag(
            ProjectionHook::MessageCleanup.id().to_owned()
        ))
    );
    let mut mismatched = valid.clone();
    mismatched["protocol_version"] = json!("limits.v2");
    assert_eq!(
        RuntimeManifest::parse(&mismatched),
        Err(ManifestRefusal::ProtocolMismatch {
            manifest: "limits.v2".to_owned(),
            identity: "limits.v1".to_owned(),
        }),
        "changed limits carry a new protocol version, which the old identity does not name"
    );

    // A manifest for another projection identity denies the hook it enables.
    let mut evaluator = passing();
    evaluator.manifest = parsed.clone();
    evaluator.manifest.identity.embedding_model = "other-model".to_owned();
    assert_eq!(
        evaluator.judge(ProjectionHook::MessageCleanup),
        Err(Denial::ManifestIdentity)
    );
    // A limit below the observed requirement denies at the resource gate.
    let mut evaluator = passing();
    evaluator
        .evidence
        .resource
        .as_mut()
        .unwrap()
        .decoded_heap_high_water_bytes = 7;
    evaluator
        .manifest
        .limits
        .insert("decoded_heap_high_water_bytes".to_owned(), 6);
    assert_eq!(
        evaluator.judge(ProjectionHook::MessageCleanup),
        Err(Denial::LimitExceeded {
            limit: "decoded_heap_high_water_bytes".to_owned(),
            observed: 7,
            max: 6,
        })
    );
    evaluator
        .manifest
        .limits
        .insert("decoded_heap_high_water_bytes".to_owned(), 7);
    evaluator.judge(ProjectionHook::MessageCleanup).unwrap();
}

/// AC2, AC3: each evidence dimension denies on its own — absent, failed, stale, unsupported, mismatched, or a disabled flag — and a passing packet admits at every entry point.
#[test]
fn each_invalid_evidence_dimension_denies_on_its_own() {
    let hook = ProjectionHook::EmbeddingBackfill;
    passing().judge(hook).unwrap();
    let gate = HookGate::closed();
    gate.install(passing());
    for entry in EntryPoint::ALL {
        let admission = gate.admit(hook, entry).unwrap();
        assert_eq!((admission.hook, admission.entry), (hook, entry));
        assert_eq!(admission.protocol_version, "limits.v1");
        assert!(!admission.invalidated.is_cancelled());
    }

    let mut absent_flag = passing();
    absent_flag.manifest.enabled.remove(&hook);
    assert_eq!(absent_flag.judge(hook), Err(Denial::Disabled(hook)));
    let mut false_flag = passing();
    false_flag.manifest.enabled.insert(hook, false);
    assert_eq!(false_flag.judge(hook), Err(Denial::Disabled(hook)));

    let mut other_identity = passing();
    other_identity.evidence.identity.tokenizer_fingerprint = "f".repeat(64);
    assert_eq!(other_identity.judge(hook), Err(Denial::EvidenceIdentity));

    let mut other_coverage = passing();
    other_coverage
        .evidence
        .coverage
        .as_mut()
        .unwrap()
        .report
        .identity
        .generation_epoch += 1;
    assert_eq!(
        other_coverage.judge(hook),
        Err(Denial::EvidenceIdentity),
        "a coverage report of another generation is not this projection's"
    );

    let mut no_coverage = passing();
    no_coverage.evidence.coverage = None;
    assert_eq!(
        no_coverage.judge(hook),
        Err(Denial::Missing(Gate::ClassCoverage))
    );
    let mut partial_coverage = passing();
    partial_coverage
        .evidence
        .coverage
        .as_mut()
        .unwrap()
        .report
        .classes
        .retain(|class| class.class != kernel::source_identity::OccurrenceClass::GitCommits);
    assert!(matches!(
        partial_coverage.judge(hook),
        Err(Denial::Failed(Gate::ClassCoverage, _))
    ));
    assert!(
        partial_coverage
            .judge(ProjectionHook::MessageCleanup)
            .is_ok(),
        "a hook that does not touch the unreported class is unaffected"
    );

    let mut stale = passing();
    stale.evidence.kernel_tip += 5;
    stale
        .manifest
        .limits
        .insert("catchup_lag_commits".to_owned(), 4);
    assert_eq!(stale.judge(hook), Err(Denial::Stale { lag: 5, max: 4 }));
    let mut ahead = passing();
    ahead.evidence.kernel_tip -= 1;
    assert_eq!(
        ahead.judge(hook),
        Err(Denial::Stale {
            lag: -1,
            max: u64::MAX
        }),
        "a checkpoint past the observed tip is not a fresh observation"
    );

    let mut charge_observer = passing();
    charge_observer.evidence.resource.as_mut().unwrap().observer =
        "logical-admission-charge".to_owned();
    assert_eq!(
        charge_observer.judge(hook),
        Err(Denial::UnapprovedObserver(
            "logical-admission-charge".to_owned()
        )),
        "an admission charge is accounting, not a heap measurement"
    );
    assert_eq!(APPROVED_OBSERVERS.len(), 1);
    let mut no_resource = passing();
    no_resource.evidence.resource = None;
    assert_eq!(
        no_resource.judge(hook),
        Err(Denial::Missing(Gate::Resource))
    );

    let required = CAPABILITIES
        .iter()
        .find(|capability| capability.disposition == CapabilityDisposition::Required)
        .unwrap();
    let mut unsupported = passing();
    unsupported.evidence.capabilities.insert(
        (HARNESSES[1].to_owned(), required.name.to_owned()),
        CapabilityEvidence::Unsupported,
    );
    assert_eq!(
        unsupported.judge(ProjectionHook::MessageCleanup),
        Err(Denial::Unsupported {
            harness: HARNESSES[1].to_owned(),
            capability: required.name.to_owned(),
        })
    );
    let mut unproved = passing();
    unproved
        .evidence
        .capabilities
        .remove(&(HARNESSES[0].to_owned(), required.name.to_owned()));
    assert_eq!(
        unproved.judge(hook),
        Err(Denial::Unsupported {
            harness: HARNESSES[0].to_owned(),
            capability: required.name.to_owned(),
        }),
        "a required capability nobody proved is unsupported"
    );
    let optional = CAPABILITIES
        .iter()
        .find(|capability| capability.disposition == CapabilityDisposition::OptionalDisabled)
        .unwrap();
    let mut optional_on = passing();
    optional_on.evidence.capabilities.insert(
        (HARNESSES[0].to_owned(), optional.name.to_owned()),
        CapabilityEvidence::Supported,
    );
    assert_eq!(
        optional_on.judge(hook),
        Err(Denial::Unsupported {
            harness: HARNESSES[0].to_owned(),
            capability: optional.name.to_owned(),
        }),
        "an optional capability proven enabled is a mismatch, not a bonus"
    );

    let mut failed_run = passing();
    failed_run.evidence.harness_runs.insert(
        HARNESSES[0].to_owned(),
        HarnessRun::Failed {
            reason: "timeout".to_owned(),
        },
    );
    assert!(matches!(
        failed_run.judge(hook),
        Err(Denial::Failed(Gate::BothHarness, _))
    ));
    let mut one_harness = passing();
    one_harness.evidence.harness_runs.remove(HARNESSES[1]);
    assert_eq!(
        one_harness.judge(hook),
        Err(Denial::Missing(Gate::BothHarness))
    );
    let mut old_run = passing();
    old_run.evidence.harness_runs.insert(
        HARNESSES[1].to_owned(),
        HarnessRun::Passed {
            identity: InvalidationIdentity::from(&identity("k", 16)),
        },
    );
    assert_eq!(
        old_run.judge(hook),
        Err(Denial::EvidenceIdentity),
        "a run under an earlier identity does not carry over"
    );
}

/// AC1, AC2: with the gate closed, every product entry path — both supervisor slices, message cleanup, git ingest, and git reconciliation — reaches the ledger under its hooks, and the ledger's hook set is exactly the product's. Nothing executes: no durable row changes, no repository is opened, no native call is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn every_entry_path_reaches_the_ledger_and_a_closed_gate_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    corpus.publish("gated", "gated text");
    let (projection, _rows) = corpus.bootstrap(dir.path());
    let projection = Arc::new(projection);
    let before = durable_work(&inspect(dir.path()));
    let gate = Arc::new(HookGate::closed());
    let engine = TestEngine::new();
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));

    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        Maintained {
            gate: Arc::clone(&gate),
            kernel: Arc::clone(&corpus.kernel),
            projection: Arc::clone(&projection),
            synapse,
            project: ProjectScope::new(PROJECT).unwrap(),
            destination: ArtifactDestination::Remote,
        },
        slice_bounds(Duration::from_secs(2)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Denied(Denial::NoManifest)
    );
    assert_eq!(
        ended(&mut events, SliceKind::Sweep).await,
        SliceOutcome::Denied(Denial::NoManifest)
    );
    supervisor.shutdown(Duration::from_secs(5)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.calls(), 0);

    let mut cleanup = MessageCleanup::new(&projection, i64::MAX);
    let report = cleanup
        .run_slice(
            &gate,
            CleanupBounds {
                page_rows: NonZeroUsize::new(4).unwrap(),
                max_pages: NonZeroUsize::new(4).unwrap(),
                max_reclaimed: NonZeroUsize::new(4).unwrap(),
            },
            &unbounded(),
        )
        .unwrap();
    assert_eq!(report.stop, Some(CleanupStop::Denied(Denial::NoManifest)));
    assert_eq!(report.inspected, 0);

    let binding = RepositoryBinding {
        repository_id: "r".repeat(64),
        path: PathBuf::from("/nonexistent/repository"),
    };
    let read_bounds = GitReadBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_object_bytes: NonZeroU64::new(4096).unwrap(),
        max_total_object_bytes: NonZeroU64::new(1 << 16).unwrap(),
    };
    assert_eq!(
        read_selection(&gate, &binding, &["0".repeat(40)], read_bounds).unwrap_err(),
        GitRefusal::Denied(Denial::NoManifest),
        "a denied ingest never opens the repository"
    );

    let kernel = KernelStore::open(dir.path().join("reconcile-kernel")).unwrap();
    let report = GitReconciler::new(&kernel)
        .run_episode(
            &gate,
            &ReconcileScope {
                binding,
                permitted_refs: vec!["refs/heads/main".to_owned()],
            },
            InventoryBounds {
                page_rows: NonZeroUsize::new(2).unwrap(),
                max_retained: NonZeroUsize::new(16).unwrap(),
                max_commits: NonZeroUsize::new(16).unwrap(),
            },
            &unbounded(),
        )
        .unwrap();
    assert_eq!(
        report.end,
        ReconcileEnd::Blocked(ReconcileBlocked::Denied(Denial::NoManifest))
    );
    assert_eq!((report.retained, report.retired), (0, 0));

    let ledger = gate.ledger();
    let asked: BTreeSet<ProjectionHook> = ledger.iter().map(|entry| entry.hook).collect();
    let all: BTreeSet<ProjectionHook> = ProjectionHook::ALL.into_iter().collect();
    assert_eq!(asked, all, "every product hook asked the gate");
    assert!(
        ledger
            .iter()
            .all(|entry| entry.verdict == Err(Denial::NoManifest))
    );
    assert!(
        ledger.iter().any(|entry| entry.entry == EntryPoint::Startup
            && entry.hook == ProjectionHook::EmbeddingBootstrap),
        "the first supervisor slice is the startup path"
    );
    assert_eq!(durable_work(&inspect(dir.path())), before);
}

/// AC3, AC4, AC5: valid test evidence lets a real backfill slice admit a native job; installing a manifest for a changed projection identity while that call is held cancels the slice's budget, the admitted row and its running native call keep their owner, and every later slice is judged against the new identity — and denied, since the manifest and evidence were gathered under the old one.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn invalidation_cancels_the_running_slice_and_keeps_admitted_work_owned() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(dir.path());
    corpus.seed();
    let object = corpus.publish("held", "held text");
    let (projection, rows) = corpus.bootstrap(dir.path());
    let occurrence = occurrence_of(&rows, &object).to_string();
    let projection = Arc::new(projection);
    let engine = TestEngine::new();
    let block = engine.block_calls();
    let _release = GateGuard(Arc::clone(&block));
    let synapse = Arc::new(component(&engine, SynapseLimits::default()));
    let gate = open_gate();
    let (sender, mut events) = unbounded_channel();
    let supervisor = EmbeddingSupervisor::new(
        Maintained {
            gate: Arc::clone(&gate),
            kernel: Arc::clone(&corpus.kernel),
            projection: Arc::clone(&projection),
            synapse: Arc::clone(&synapse),
            project: ProjectScope::new(PROJECT).unwrap(),
            destination: ArtifactDestination::Remote,
        },
        slice_bounds(Duration::from_secs(30)),
        Arc::new(|| NOW),
        sender,
    );
    let running = tokio::spawn(Arc::clone(&supervisor).run());

    // The admitted row is the slice's real effect; the engine holds the call.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while engine.calls() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(engine.calls(), 1);

    let mut changed = passing_evaluator(&identity("k", 8), 0, &ProjectionHook::ALL);
    changed.current.generation_epoch += 1;
    gate.install(changed);
    match ended(&mut events, SliceKind::Backfill).await {
        SliceOutcome::Backfill { end, admitted, .. } => {
            assert_eq!((end, admitted), (Some(Blocked::BudgetExhausted), 1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        ended(&mut events, SliceKind::Sweep).await,
        SliceOutcome::Denied(Denial::ManifestIdentity)
    );
    assert_eq!(
        ended(&mut events, SliceKind::Backfill).await,
        SliceOutcome::Denied(Denial::ManifestIdentity)
    );
    let (state, host_job): (String, Option<String>) = inspect(dir.path())
        .query_row(
            "SELECT state,host_job_id FROM embedding_jobs WHERE occurrence_id=?1",
            [&occurrence],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "admitted");
    assert_eq!(synapse.job_status(&host_job.unwrap()), Some("running"));
    assert_eq!(engine.calls(), 1, "no work ran under the invalidated grant");

    let ledger = gate.ledger();
    let verdicts: Vec<bool> = ledger.iter().map(|entry| entry.verdict.is_ok()).collect();
    assert!(
        verdicts.starts_with(&[true; 6]),
        "the first backfill's six hooks were admitted"
    );
    assert!(
        verdicts[6..].iter().all(|ok| !ok),
        "nothing after the close was admitted"
    );

    supervisor
        .shutdown(Duration::from_millis(200))
        .await
        .unwrap_err();
    TestEngine::release(&block);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while engine.completed() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let report = supervisor.shutdown(Duration::from_secs(5)).await.unwrap();
    assert_eq!(report.held_results, 1);
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
}
