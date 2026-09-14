//! Exercises projection admission from the persisted records through a real hook's effect.

mod support;

use std::fs;
use std::num::NonZeroUsize;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use daemon::coverage::{ProjectionCoverage, observe_coverage};
use daemon::message_cleanup::{CleanupBounds, CleanupStop, MessageCleanup};
use daemon::projection_admission::{
    ADMISSION_DIR, AdmissionInputs, Closed, EVIDENCE_RECORD, InputRefusal, MANIFEST_RECORD,
    ProjectionAdmission, Refresh, SelectedProjection,
};
use daemon::projection_gates::{
    APPROVED_OBSERVERS, CAPABILITIES, CapabilityDisposition, Denial, EntryPoint, Gate, HARNESSES,
    HookGate, ManifestRefusal, ProjectionHook, REQUIRED_LIMITS, Renewal, ResourceEvidence,
};
use daemon::projection_lifecycle::{MAX_RECORD_BYTES, ProjectionLifecycle};
use kernel::applicability::EvalBudget;
use kernel::{ArtifactDestination, ProjectScope};
use retrieval::ProjectionIdentity;
use retrieval::coverage::CoverageBounds;
use serde_json::{Value, json};
use support::embedding_fixtures::{
    Corpus, PROJECT, generation, kernel_incarnation_id, occurrence_of, tombstone,
};
use support::kernel_daemon::KernelDaemon;
use support::projection_gate::{empty_coverage, identity, passing_evaluator};

const LAG_LIMIT: u64 = 4;
const LIMIT: u64 = 1_000_000;
const HEAP_BYTES: u64 = 1024;

fn manifest_json(identity: &ProjectionIdentity, enabled: &[ProjectionHook]) -> Value {
    let limits: serde_json::Map<String, Value> = REQUIRED_LIMITS
        .into_iter()
        .map(|name| {
            let value = if name == "catchup_lag_commits" {
                LAG_LIMIT
            } else {
                LIMIT
            };
            (name.to_owned(), json!(value))
        })
        .collect();
    let hooks: serde_json::Map<String, Value> = ProjectionHook::ALL
        .iter()
        .map(|hook| {
            (
                hook.id().to_owned(),
                json!({ "enabled": enabled.contains(hook) }),
            )
        })
        .collect();
    json!({
        "protocol_version": identity.limit_manifest_protocol_version,
        "invalidation_identity": invalidation_json(identity),
        "limits": limits,
        "hooks": hooks,
    })
}

fn invalidation_json(identity: &ProjectionIdentity) -> Value {
    json!({
        "schema_version": identity.schema_version,
        "tokenizer_fingerprint": identity.tokenizer_fingerprint,
        "embedding_model": identity.embedding_model,
        "projection_policy_version": identity.projection_policy_version,
        "identity_contract_version": identity.identity_contract_version,
        "limit_manifest_protocol_version": identity.limit_manifest_protocol_version,
        "vector_dimension": identity.vector_dimension,
        "generation_epoch": identity.generation_epoch,
    })
}

/// A campaign record whose every dimension passes under `identity`.
fn campaign_json(identity: &ProjectionIdentity) -> Value {
    let proved: serde_json::Map<String, Value> = CAPABILITIES
        .iter()
        .map(|capability| {
            let outcome = match capability.disposition {
                CapabilityDisposition::Required => "supported",
                CapabilityDisposition::OptionalDisabled => "unsupported",
            };
            (capability.name.to_owned(), json!(outcome))
        })
        .collect();
    let capabilities: serde_json::Map<String, Value> = HARNESSES
        .iter()
        .map(|harness| ((*harness).to_owned(), Value::Object(proved.clone())))
        .collect();
    let harness_runs: serde_json::Map<String, Value> = HARNESSES
        .iter()
        .map(|harness| ((*harness).to_owned(), passed_run_json(identity)))
        .collect();
    json!({
        "invalidation_identity": invalidation_json(identity),
        "resource": {
            "observer": APPROVED_OBSERVERS[0],
            "decoded_heap_high_water_bytes": HEAP_BYTES,
        },
        "capabilities": capabilities,
        "harness_runs": harness_runs,
    })
}

fn passed_run_json(identity: &ProjectionIdentity) -> Value {
    json!({
        "outcome": "passed",
        "invalidation_identity": invalidation_json(identity),
    })
}

fn write_record(home: &Path, record: &str, bytes: &[u8]) {
    let dir = home.join(ADMISSION_DIR);
    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&dir) {
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    }
    let path = dir.join(record);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn write_records(home: &Path, manifest: &Value, campaign: &Value) {
    write_record(
        home,
        MANIFEST_RECORD,
        &serde_json::to_vec_pretty(manifest).unwrap(),
    );
    write_record(
        home,
        EVIDENCE_RECORD,
        &serde_json::to_vec_pretty(campaign).unwrap(),
    );
}

/// Refreshes `admission` for `identity` with an empty projection's coverage observed at `tip`.
fn refresh_at(admission: &ProjectionAdmission, identity: &ProjectionIdentity, tip: i64) -> Refresh {
    let coverage = empty_coverage(identity, tip);
    admission.refresh(Some(SelectedProjection {
        identity,
        coverage: Some(&coverage),
    }))
}

fn refresh_with(
    admission: &ProjectionAdmission,
    identity: &ProjectionIdentity,
    coverage: Option<&ProjectionCoverage>,
) -> Refresh {
    admission.refresh(Some(SelectedProjection { identity, coverage }))
}

/// Every hook's verdict at every entry point.
fn verdicts(gate: &HookGate) -> Vec<Result<(), Denial>> {
    ProjectionHook::ALL
        .iter()
        .flat_map(|hook| {
            EntryPoint::ALL
                .iter()
                .map(|entry| gate.admit(*hook, *entry).map(|_| ()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn all_denied(gate: &HookGate, expected: impl Fn(ProjectionHook) -> Denial) {
    for (hook, verdict) in ProjectionHook::ALL.iter().flat_map(|hook| {
        EntryPoint::ALL
            .iter()
            .map(move |entry| (*hook, gate.admit(*hook, *entry).map(|_| ())))
    }) {
        assert_eq!(verdict, Err(expected(hook)), "{hook:?}");
    }
}

fn unbounded() -> EvalBudget {
    EvalBudget::new(
        Some(std::time::Instant::now() + Duration::from_secs(30)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

/// The reader accepts only the caller's own owner-only regular records under an owner-only directory, under the size cap and of their schemas, and maps them onto the evaluator the test support builds by hand; the manifest's own refusals pass through, and a harness or capability the contract does not name refuses the campaign record.
#[test]
fn records_must_be_the_callers_own_regular_files_of_their_schemas() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let identity = identity("k", 8);
    let manifest = manifest_json(&identity, &ProjectionHook::ALL);
    let campaign = campaign_json(&identity);

    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Missing(MANIFEST_RECORD)
    );
    write_record(
        home,
        MANIFEST_RECORD,
        &serde_json::to_vec(&manifest).unwrap(),
    );
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Missing(EVIDENCE_RECORD)
    );
    write_records(home, &manifest, &campaign);
    let mut expected = passing_evaluator(&identity, 10, &ProjectionHook::ALL);
    for (name, limit) in expected.manifest.limits.iter_mut() {
        *limit = if name == "catchup_lag_commits" {
            LAG_LIMIT
        } else {
            LIMIT
        };
    }
    expected.evidence.resource = Some(ResourceEvidence {
        observer: APPROVED_OBSERVERS[0].to_owned(),
        decoded_heap_high_water_bytes: HEAP_BYTES,
    });
    assert_eq!(
        AdmissionInputs::read(home)
            .unwrap()
            .evaluator(&identity, Some(empty_coverage(&identity, 10))),
        expected,
        "the records map onto the hand-built evaluator"
    );

    let dir = home.join(ADMISSION_DIR);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(
        matches!(
            AdmissionInputs::read(home).unwrap_err(),
            InputRefusal::Unreadable { record, .. } if record == MANIFEST_RECORD
        ),
        "a directory others may search is not trusted"
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    fs::rename(&dir, elsewhere.path().join("moved")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path().join("moved"), &dir).unwrap();
    assert!(
        matches!(
            AdmissionInputs::read(home).unwrap_err(),
            InputRefusal::Unreadable { record, .. } if record == MANIFEST_RECORD
        ),
        "a symlinked directory is not followed"
    );
    fs::remove_file(&dir).unwrap();
    fs::rename(elsewhere.path().join("moved"), &dir).unwrap();
    AdmissionInputs::read(home).unwrap();

    let evidence_path = dir.join(EVIDENCE_RECORD);
    fs::set_permissions(&evidence_path, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Refused {
            record: EVIDENCE_RECORD,
            reason: "not the caller's own owner-only regular file",
        }
    );
    fs::set_permissions(&evidence_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&evidence_path).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&evidence_path)
        .unwrap();
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Refused {
            record: EVIDENCE_RECORD,
            reason: "not the caller's own owner-only regular file",
        },
        "a directory in place of the record"
    );
    fs::remove_dir(&evidence_path).unwrap();
    std::os::unix::fs::symlink(
        home.join(ADMISSION_DIR).join(MANIFEST_RECORD),
        &evidence_path,
    )
    .unwrap();
    assert!(
        matches!(
            AdmissionInputs::read(home).unwrap_err(),
            InputRefusal::Unreadable { record, .. } if record == EVIDENCE_RECORD
        ),
        "a symlink is not followed"
    );
    fs::remove_file(&evidence_path).unwrap();

    let mut padded = serde_json::to_vec(&campaign).unwrap();
    padded.resize(MAX_RECORD_BYTES as usize, b' ');
    write_record(home, EVIDENCE_RECORD, &padded);
    AdmissionInputs::read(home).expect("a record of exactly the cap is read");
    padded.push(b' ');
    write_record(home, EVIDENCE_RECORD, &padded);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Refused {
            record: EVIDENCE_RECORD,
            reason: "over the size cap",
        }
    );
    write_record(home, EVIDENCE_RECORD, b"{ not json");
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Malformed(EVIDENCE_RECORD)
    );

    let mut extra_field = campaign.clone();
    extra_field["approval_digest"] = json!("abc");
    write_records(home, &manifest, &extra_field);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Malformed(EVIDENCE_RECORD),
        "campaign provenance stays outside the record"
    );
    let mut bad_outcome = campaign.clone();
    bad_outcome["harness_runs"]["pi"] = json!({ "outcome": "skipped" });
    write_records(home, &manifest, &bad_outcome);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Malformed(EVIDENCE_RECORD)
    );
    let mut unbound_pass = campaign.clone();
    unbound_pass["harness_runs"]["pi"] = json!({ "outcome": "passed" });
    write_records(home, &manifest, &unbound_pass);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Malformed(EVIDENCE_RECORD),
        "a passed run names the identity it ran under"
    );
    let mut annotated_run = campaign.clone();
    annotated_run["harness_runs"]["pi"]["run_id"] = json!("r1");
    write_records(home, &manifest, &annotated_run);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Malformed(EVIDENCE_RECORD),
        "run provenance stays outside the record"
    );
    let mut unknown_harness = campaign.clone();
    unknown_harness["harness_runs"]["cursor"] = passed_run_json(&identity);
    write_records(home, &manifest, &unknown_harness);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::UnknownHarness("cursor".to_owned())
    );
    let mut long_harness = campaign.clone();
    long_harness["harness_runs"][format!("{}\n{}", "h".repeat(70), "tail")] =
        passed_run_json(&identity);
    write_records(home, &manifest, &long_harness);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::UnknownHarness("h".repeat(64)),
        "a refusal echoes a bounded, escaped prefix of the key"
    );
    let mut unknown_capability = campaign.clone();
    unknown_capability["capabilities"]["pi"]["host_mural_rendering"] = json!("supported");
    write_records(home, &manifest, &unknown_capability);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::UnknownCapability("host_mural_rendering".to_owned())
    );

    let mut missing_limit = manifest.clone();
    missing_limit["limits"]
        .as_object_mut()
        .unwrap()
        .remove("pending_bytes");
    write_records(home, &missing_limit, &campaign);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Manifest(ManifestRefusal::MissingLimit("pending_bytes".to_owned()))
    );
    let mut text_limit = manifest.clone();
    text_limit["limits"]["pending_bytes"] = json!("64");
    write_records(home, &text_limit, &campaign);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Manifest(ManifestRefusal::NonNumericLimit("pending_bytes".to_owned()))
    );
    let mut old_protocol = manifest.clone();
    old_protocol["protocol_version"] = json!("limits.v0");
    write_records(home, &old_protocol, &campaign);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Manifest(ManifestRefusal::ProtocolMismatch {
            manifest: "limits.v0".to_owned(),
            identity: "limits.v1".to_owned(),
        }),
        "a changed limit ships under a new protocol version on both records"
    );

    let mut long_protocol = manifest.clone();
    long_protocol["protocol_version"] = json!(format!("{}\n{}", "v".repeat(70), "tail"));
    write_records(home, &long_protocol, &campaign);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Manifest(ManifestRefusal::ProtocolMismatch {
            manifest: "v".repeat(64),
            identity: "limits.v1".to_owned(),
        }),
        "a manifest refusal echoes the same bounded prefix a campaign refusal does"
    );
    let mut unknown_hook = manifest.clone();
    unknown_hook["hooks"]["hook\nname\u{1b}[31m"] = json!({ "enabled": true });
    write_records(home, &unknown_hook, &campaign);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Manifest(ManifestRefusal::UnknownHook(
            "hook\\nname\\u{1b}[31m".to_owned()
        )),
        "a manifest key is escaped before it reaches a log line"
    );
    let mut unknown_limit = manifest.clone();
    unknown_limit["limits"][format!("{}\n", "l".repeat(70))] = json!(1);
    write_records(home, &unknown_limit, &campaign);
    assert_eq!(
        AdmissionInputs::read(home).unwrap_err(),
        InputRefusal::Manifest(ManifestRefusal::UnknownLimit("l".repeat(64)))
    );
}

/// AC2, AC3: a refresh without both records, or without a selected projection, leaves the gate closed; valid records with a selected projection admit every hook at every entry point, and each invalid dimension of the campaign record, the manifest's flags, or the live coverage denies every hook without a single grant in the ledger.
#[test]
fn refresh_installs_only_for_valid_records_and_a_selected_projection() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let current = identity("k", 8);
    let admission = ProjectionAdmission::for_home(home);
    let gate = Arc::clone(admission.gate());

    assert_eq!(
        admission.refresh(None),
        Refresh::Closed(Closed::NoProjection),
        "nothing selected is decided before any record is read"
    );
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Closed(Closed::Inputs(InputRefusal::Missing(MANIFEST_RECORD)))
    );
    all_denied(&gate, |_| Denial::NoManifest);

    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    assert_eq!(
        admission.refresh(None),
        Refresh::Closed(Closed::NoProjection)
    );
    all_denied(&gate, |_| Denial::NoManifest);

    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Installed(Renewal::Kept)
    );
    assert!(verdicts(&gate).iter().all(Result::is_ok));
    let granted = gate
        .ledger()
        .iter()
        .filter(|entry| entry.verdict.is_ok())
        .count();
    assert_eq!(granted, ProjectionHook::ALL.len() * EntryPoint::ALL.len());

    let mut other_epoch = current.clone();
    other_epoch.generation_epoch += 1;
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&other_epoch),
    );
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Installed(Renewal::Invalidated)
    );
    all_denied(&gate, |_| Denial::EvidenceIdentity);

    write_records(
        home,
        &manifest_json(&other_epoch, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| Denial::ManifestIdentity);

    write_records(
        home,
        &manifest_json(&current, &[]),
        &campaign_json(&current),
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, Denial::Disabled);

    let mut failed_run = campaign_json(&current);
    failed_run["harness_runs"]["pi"] = json!({ "outcome": "failed" });
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &failed_run,
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| {
        Denial::Failed(Gate::BothHarness, "pi".to_owned())
    });

    let mut earlier_run = campaign_json(&current);
    earlier_run["harness_runs"]["pi"] = passed_run_json(&{
        let mut earlier = current.clone();
        earlier.generation_epoch -= 1;
        earlier
    });
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &earlier_run,
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| Denial::EvidenceIdentity);

    let mut unmeasured = campaign_json(&current);
    unmeasured["resource"] = Value::Null;
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &unmeasured,
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| Denial::Missing(Gate::Resource));

    let mut over_limit = campaign_json(&current);
    over_limit["resource"]["decoded_heap_high_water_bytes"] = json!(LIMIT + 1);
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &over_limit,
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| Denial::LimitExceeded {
        limit: "decoded_heap_high_water_bytes".to_owned(),
        observed: LIMIT + 1,
        max: LIMIT,
    });

    let mut charge_observer = campaign_json(&current);
    charge_observer["resource"]["observer"] = json!("logical-admission-charges");
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &charge_observer,
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| {
        Denial::UnapprovedObserver("logical-admission-charges".to_owned())
    });

    let mut unproved = campaign_json(&current);
    unproved["capabilities"]["opencode"]
        .as_object_mut()
        .unwrap()
        .remove("durable_session_identity");
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &unproved,
    );
    let _ = refresh_at(&admission, &current, 10);
    all_denied(&gate, |_| Denial::Unsupported {
        harness: "opencode".to_owned(),
        capability: "durable_session_identity".to_owned(),
    });

    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    let _ = refresh_with(&admission, &current, None);
    all_denied(&gate, |_| Denial::Missing(Gate::ClassCoverage));

    let mut stale = empty_coverage(&current, 20);
    stale.report.checkpoint.checkpoint_commit_seq = 20 - LAG_LIMIT as i64 - 1;
    let _ = refresh_with(&admission, &current, Some(&stale));
    all_denied(&gate, |_| Denial::Stale {
        lag: LAG_LIMIT as i64 + 1,
        max: LAG_LIMIT,
    });

    let ledger = gate.ledger();
    assert_eq!(
        ledger.iter().filter(|entry| entry.verdict.is_ok()).count(),
        granted,
        "no denied dimension produced a grant"
    );
}

/// AC2, AC4: fresh coverage that still admits the running work keeps its grant; a manifest that withdraws a hook, or coverage that has gone stale, cancels every grant issued before it.
#[test]
fn renewal_keeps_grants_fresh_evidence_admits_and_cancels_withdrawn_ones() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let current = identity("k", 8);
    let admission = ProjectionAdmission::for_home(home);
    let gate = admission.gate();
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Installed(Renewal::Kept)
    );
    let grant = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Startup)
        .unwrap();

    assert_eq!(
        refresh_at(&admission, &current, 13),
        Refresh::Installed(Renewal::Kept)
    );
    assert!(!grant.invalidated.is_cancelled());
    gate.check_limits(&grant, &(&current).into(), &[("pending_count", 1)])
        .unwrap();

    write_records(
        home,
        &manifest_json(
            &current,
            &ProjectionHook::ALL
                .into_iter()
                .filter(|hook| *hook != ProjectionHook::GitLeases)
                .collect::<Vec<_>>(),
        ),
        &campaign_json(&current),
    );
    assert_eq!(
        refresh_at(&admission, &current, 13),
        Refresh::Installed(Renewal::Invalidated),
        "a withdrawn hook cancels grants even for hooks still enabled"
    );
    assert!(grant.invalidated.is_cancelled());
    assert_eq!(
        gate.check_limits(&grant, &(&current).into(), &[("pending_count", 1)]),
        Err(Denial::Invalidated)
    );
    let grant = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    assert_eq!(
        gate.admit(ProjectionHook::GitLeases, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::Disabled(ProjectionHook::GitLeases)
    );

    let mut stale = empty_coverage(&current, 30);
    stale.report.checkpoint.checkpoint_commit_seq = 30 - LAG_LIMIT as i64 - 1;
    assert_eq!(
        refresh_with(&admission, &current, Some(&stale)),
        Refresh::Installed(Renewal::Invalidated),
        "a startup grant does not survive coverage that fell behind"
    );
    assert!(grant.invalidated.is_cancelled());
    assert_eq!(
        gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::Stale {
            lag: LAG_LIMIT as i64 + 1,
            max: LAG_LIMIT
        }
    );

    assert_eq!(
        refresh_at(&admission, &current, 30),
        Refresh::Installed(Renewal::Kept),
        "no grant was outstanding to invalidate"
    );
    let grant = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    let mut tightened = manifest_json(&current, &ProjectionHook::ALL);
    tightened["limits"]["pending_count"] = json!(LIMIT - 1);
    write_records(home, &tightened, &campaign_json(&current));
    assert_eq!(
        refresh_at(&admission, &current, 30),
        Refresh::Installed(Renewal::Invalidated),
        "a changed limit cancels grants even when it denies no hook"
    );
    assert!(grant.invalidated.is_cancelled());

    let grant = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    assert_eq!(
        admission.refresh(None),
        Refresh::Closed(Closed::NoProjection)
    );
    assert!(
        grant.invalidated.is_cancelled(),
        "deselection cancels grants"
    );
    assert_eq!(
        gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::NoManifest
    );

    assert_eq!(
        refresh_at(&admission, &current, 30),
        Refresh::Installed(Renewal::Kept)
    );
    let grant = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    fs::remove_file(home.join(ADMISSION_DIR).join(EVIDENCE_RECORD)).unwrap();
    assert_eq!(
        refresh_at(&admission, &current, 30),
        Refresh::Closed(Closed::Inputs(InputRefusal::Missing(EVIDENCE_RECORD)))
    );
    assert!(grant.invalidated.is_cancelled());
    assert_eq!(
        gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::NoManifest
    );
}

/// AC2, AC4: a disabled gate takes the refreshed evaluator for cleanup and recovery but admits nothing until authorized recovery, and a closed owner refreshes nothing.
#[test]
fn a_disabled_gate_and_a_closed_owner_admit_nothing_after_a_refresh() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let current = identity("k", 8);
    let admission = ProjectionAdmission::for_home(home);
    let gate = admission.gate();
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Installed(Renewal::Kept)
    );
    let grant = gate
        .admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
        .unwrap();
    ProjectionLifecycle::open(home)
        .unwrap()
        .disable(gate, 1_000)
        .unwrap();
    assert!(grant.invalidated.is_cancelled());
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Installed(Renewal::Disabled)
    );
    all_denied(gate, |_| Denial::RecoveryRequired);

    admission.close();
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Closed(Closed::ShutDown)
    );
    assert_eq!(
        gate.admit(ProjectionHook::EmbeddingBackfill, EntryPoint::Dispatch)
            .unwrap_err(),
        Denial::RecoveryRequired
    );
}

/// AC4: the records persist across a restart but the grant does not: a reopened owner denies until it refreshes, and the refreshed evaluator equals the one the records described before. The running daemon binds its owner to the store's data home, starts closed, and stays closed when no family is selected, whatever the records say.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_reopens_the_gate_closed_until_it_refreshes() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let current = identity("k", 8);
    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    let before = {
        let admission = ProjectionAdmission::for_home(home);
        let _ = refresh_at(&admission, &current, 10);
        admission
            .gate()
            .admit(ProjectionHook::GitIngest, EntryPoint::Dispatch)
            .unwrap();
        AdmissionInputs::read(home)
            .unwrap()
            .evaluator(&current, Some(empty_coverage(&current, 10)))
    };
    let admission = ProjectionAdmission::for_home(home);
    assert_eq!(
        admission
            .gate()
            .admit(ProjectionHook::GitIngest, EntryPoint::Startup)
            .unwrap_err(),
        Denial::NoManifest
    );
    assert_eq!(
        refresh_at(&admission, &current, 10),
        Refresh::Installed(Renewal::Kept)
    );
    assert_eq!(
        AdmissionInputs::read(home)
            .unwrap()
            .evaluator(&current, Some(empty_coverage(&current, 10))),
        before
    );

    let daemon = KernelDaemon::start().await;
    let data_home = daemon.data_home().to_owned();
    // The owner binds after the kernel reports ready, so the accessor may trail the start by a moment.
    let started = std::time::Instant::now();
    let admission = loop {
        if let Some(admission) = daemon.handler().projection_admission() {
            break admission;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a SQLite store binds the admission owner"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    all_denied(admission.gate(), |_| Denial::NoManifest);
    write_records(
        &data_home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    assert_eq!(
        admission.refresh(None),
        Refresh::Closed(Closed::NoProjection),
        "records alone select no family"
    );
    all_denied(admission.gate(), |_| Denial::NoManifest);
    assert_eq!(
        refresh_at(admission, &current, 0),
        Refresh::Installed(Renewal::Kept)
    );
    let grant = admission
        .gate()
        .admit(ProjectionHook::MessageCleanup, EntryPoint::Dispatch)
        .unwrap();
    daemon.shutdown().await;
    assert!(grant.invalidated.is_cancelled(), "shutdown closes the gate");
}

/// AC1: valid records with coverage observed on a real projection let message cleanup inspect the projection; the same slice under records that enable no hook inspects nothing, so an evaluator that always denied would fail this control.
#[test]
fn record_fed_evidence_reaches_message_cleanup_and_all_disabled_records_reach_nothing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let corpus = Corpus::open(home);
    corpus.seed();
    let retired = corpus.publish("retired", "retired text");
    let (projection, rows) = corpus.bootstrap(home);
    tombstone(
        &projection,
        occurrence_of(&rows, &retired),
        corpus.tip() + 1,
    );
    let incarnation = kernel_incarnation_id(home);
    let current = support::embedding_fixtures::identity(&incarnation);
    let coverage: ProjectionCoverage = observe_coverage(
        &projection,
        &corpus.kernel,
        &incarnation,
        &ProjectScope::new(PROJECT).unwrap(),
        ArtifactDestination::Remote,
        &generation(),
        CoverageBounds {
            max_live_per_class: NonZeroUsize::new(64).unwrap(),
            max_tombstoned_per_class: NonZeroUsize::new(64).unwrap(),
        },
    )
    .unwrap()
    .unwrap();
    let admission = ProjectionAdmission::for_home(home);
    let bounds = CleanupBounds {
        page_rows: NonZeroUsize::new(4).unwrap(),
        max_pages: NonZeroUsize::new(4).unwrap(),
        max_reclaimed: NonZeroUsize::new(4).unwrap(),
    };

    write_records(
        home,
        &manifest_json(&current, &[]),
        &campaign_json(&current),
    );
    assert_eq!(
        refresh_with(&admission, &current, Some(&coverage)),
        Refresh::Installed(Renewal::Kept)
    );
    let report = MessageCleanup::new(&projection, incarnation.clone(), i64::MAX)
        .run_slice(admission.gate(), bounds, &unbounded())
        .unwrap();
    assert_eq!(
        report.stop,
        Some(CleanupStop::Denied(Denial::Disabled(
            ProjectionHook::MessageCleanup
        )))
    );
    assert_eq!(report.inspected, 0);

    write_records(
        home,
        &manifest_json(&current, &ProjectionHook::ALL),
        &campaign_json(&current),
    );
    assert_eq!(
        refresh_with(&admission, &current, Some(&coverage)),
        Refresh::Installed(Renewal::Invalidated)
    );
    let report = MessageCleanup::new(&projection, incarnation, i64::MAX)
        .run_slice(admission.gate(), bounds, &unbounded())
        .unwrap();
    assert_eq!(report.stop, None, "the admitted slice ran to its end");
    assert!(report.inspected > 0, "the slice inspected the projection");
    let ledger = admission.gate().ledger();
    assert_eq!(ledger.len(), 2);
    assert_eq!(
        ledger[0].verdict,
        Err(Denial::Disabled(ProjectionHook::MessageCleanup))
    );
    assert_eq!(ledger[1].verdict, Ok(()));
}
