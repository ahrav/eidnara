//! The compression gate against a hand-built passing campaign, one dimension broken at a time. Every expected denial is the contract's; a report fixture proves the evaluator and authorizes nothing.

mod support;

use daemon::projection_gates::{
    COMPRESSED_ACTIVATION_ID, COMPRESSION_CRITERIA, CompressionRecord, Denial, EvidenceEvaluator,
    Gate, HARNESSES, HookGate, Outcome, ProjectionHook, Renewal, RuntimeManifest, TRACE_STAGES,
    TraceKind, VECTOR_LIMITS,
};
use serde_json::json;
use support::projection_gate::{identity, passing_evaluator};

fn passing() -> EvidenceEvaluator {
    passing_evaluator(&identity("k", 8), 10, &ProjectionHook::ALL)
}

fn judge(mutate: impl FnOnce(&mut EvidenceEvaluator)) -> Result<(), Denial> {
    let mut evaluator = passing();
    mutate(&mut evaluator);
    evaluator.judge_compressed_activation()
}

fn compression(
    evaluator: &mut EvidenceEvaluator,
) -> &mut daemon::projection_gates::CompressionEvidence {
    match &mut evaluator.evidence.compression {
        CompressionRecord::Campaign(evidence) => evidence.as_mut(),
        other => panic!("the passing evaluator carries a campaign, not {other:?}"),
    }
}

#[test]
fn a_correctly_bound_campaign_admits_and_every_broken_dimension_denies_on_its_own() {
    assert_eq!(judge(|_| {}), Ok(()));

    // The flag alone refuses, whatever the evidence says.
    assert_eq!(
        judge(|e| e.manifest.compressed_activation = false),
        Err(Denial::CompressionDisabled)
    );
    assert_eq!(
        judge(|e| e.evidence.compression = CompressionRecord::Absent),
        Err(Denial::Missing(Gate::Compression))
    );
    assert_eq!(
        judge(|e| e.evidence.compression = CompressionRecord::Malformed),
        Err(Denial::Failed(
            Gate::Compression,
            "the compression section is malformed".to_owned()
        )),
        "a section the gate cannot read denies activation and nothing else"
    );
    assert_eq!(
        judge(|e| e.binding = None),
        Err(Denial::Missing(Gate::Compression)),
        "a daemon that cannot name its own binding refuses rather than guessing"
    );
    assert_eq!(
        judge(|e| compression(e).identity.generation_epoch += 1),
        Err(Denial::EvidenceIdentity),
        "a campaign under another projection identity is stale"
    );
    assert_eq!(
        judge(|e| compression(e).binding.hardware = "other-hardware".to_owned()),
        Err(Denial::EvidenceIdentity)
    );
    assert_eq!(
        judge(|e| compression(e)
            .binding
            .harnesses
            .insert("pi".to_owned(), "older".to_owned())
            .map(|_| ())
            .unwrap()),
        Err(Denial::EvidenceIdentity),
        "a harness version the daemon does not run under is a mismatch"
    );
    for harness in HARNESSES {
        assert_eq!(
            judge(|e| {
                e.binding.as_mut().unwrap().harnesses.remove(harness);
                compression(e).binding.harnesses.remove(harness);
            }),
            Err(Denial::Failed(
                Gate::Compression,
                "the binding does not name exactly one version per harness".to_owned()
            )),
            "a harness version both sides omit is unbound, not matching"
        );
    }
    assert_eq!(
        judge(|e| compression(e).revoked = true),
        Err(Denial::Revoked)
    );
    for limit in VECTOR_LIMITS {
        assert_eq!(
            judge(|e| {
                e.manifest.limits.insert(limit.to_owned(), 7);
            }),
            Err(Denial::LimitChanged {
                limit: limit.to_owned(),
                evidence: u64::MAX,
                manifest: 7
            }),
            "a cap changed since the campaign invalidates it"
        );
        assert_eq!(
            judge(|e| {
                compression(e).limits.remove(limit);
            }),
            Err(Denial::Failed(
                Gate::Compression,
                format!("the campaign does not record {limit}")
            ))
        );
        assert_eq!(
            judge(|e| {
                e.manifest.limits.remove(limit);
            }),
            Err(Denial::Failed(
                Gate::Resource,
                format!("limit {limit} is absent")
            )),
            "a manifest without the cap fails closed"
        );
    }
    for criterion in COMPRESSION_CRITERIA {
        assert_eq!(
            judge(|e| {
                compression(e)
                    .criteria
                    .insert(criterion.to_owned(), Outcome::Failed);
            }),
            Err(Denial::Failed(
                Gate::Compression,
                format!("criterion {criterion} failed")
            ))
        );
        assert_eq!(
            judge(|e| {
                compression(e).criteria.remove(criterion);
            }),
            Err(Denial::Failed(
                Gate::Compression,
                format!("criterion {criterion} is missing")
            ))
        );
    }
    for harness in ["opencode", "pi"] {
        let unsupported = Err(Denial::Unsupported {
            harness: harness.to_owned(),
            capability: "compressed_full_path_trace".to_owned(),
        });
        assert_eq!(
            judge(|e| {
                compression(e).traces.remove(harness);
            }),
            unsupported,
            "each harness must supply its own trace"
        );
        for kind in [TraceKind::Simulated, TraceKind::ReportOnly] {
            assert_eq!(
                judge(|e| compression(e).traces.get_mut(harness).unwrap().kind = kind),
                unsupported,
                "a {kind:?} trace proves nothing about the path"
            );
        }
        for stage in TRACE_STAGES {
            assert_eq!(
                judge(|e| {
                    compression(e)
                        .traces
                        .get_mut(harness)
                        .unwrap()
                        .stages
                        .remove(stage);
                }),
                unsupported,
                "a trace without {stage} is not the full path"
            );
        }
    }
    // The gates every dense hook passes still apply: a failed harness run or a stale coverage report denies before the campaign is read.
    assert_eq!(
        judge(|e| {
            e.evidence.harness_runs.insert(
                "pi".to_owned(),
                daemon::projection_gates::HarnessRun::Failed,
            );
        }),
        Err(Denial::Failed(Gate::BothHarness, "pi".to_owned()))
    );
    assert_eq!(
        judge(|e| e.evidence.coverage = None),
        Err(Denial::Missing(Gate::ClassCoverage))
    );
}

#[test]
fn the_gate_admits_compressed_activation_only_under_an_installed_evaluator_and_withdraws_it_on_change()
 {
    let gate = HookGate::closed();
    assert_eq!(
        gate.admit_compressed_activation().unwrap_err(),
        Denial::NoManifest
    );
    gate.install(passing());
    let grant = gate.admit_compressed_activation().unwrap();
    assert!(!grant.invalidated.is_cancelled());
    let mut disabled = passing();
    disabled.manifest.compressed_activation = false;
    gate.install(disabled);
    assert!(
        grant.invalidated.is_cancelled(),
        "a new manifest withdraws the grant"
    );
    assert_eq!(
        gate.admit_compressed_activation().unwrap_err(),
        Denial::CompressionDisabled
    );
    gate.close();
    assert_eq!(
        gate.admit_compressed_activation().unwrap_err(),
        Denial::NoManifest
    );

    // On the refresh path, evidence alone can withdraw a grant: a revoked campaign under an unchanged manifest cancels it, and unchanged evidence keeps it.
    let gate = HookGate::closed();
    gate.install(passing());
    let grant = gate.admit_compressed_activation().unwrap();
    assert_eq!(gate.renew_for_test(passing()), Renewal::Kept);
    assert!(!grant.invalidated.is_cancelled());
    let mut revoked = passing();
    compression(&mut revoked).revoked = true;
    assert_eq!(gate.renew_for_test(revoked), Renewal::Invalidated);
    assert!(grant.invalidated.is_cancelled());
    assert_eq!(
        gate.admit_compressed_activation().unwrap_err(),
        Denial::Revoked
    );
    // Restored evidence admits again; a refusal turning into an admission withdraws nothing, so hook grants stand.
    let hook_grant = gate
        .admit(
            ProjectionHook::EmbeddingBootstrap,
            daemon::projection_gates::EntryPoint::Explicit,
        )
        .unwrap();
    assert_eq!(gate.renew_for_test(passing()), Renewal::Kept);
    assert!(!hook_grant.invalidated.is_cancelled());
    assert!(gate.admit_compressed_activation().is_ok());

    // A campaign that passes under a new daemon binding admits, but the grant issued under the old binding does not carry over to it.
    let grant = gate.admit_compressed_activation().unwrap();
    let mut rebound = passing();
    rebound.binding.as_mut().unwrap().hardware = "other-hardware".to_owned();
    compression(&mut rebound).binding.hardware = "other-hardware".to_owned();
    assert_eq!(rebound.judge_compressed_activation(), Ok(()));
    assert_eq!(
        gate.renew_for_test(rebound),
        Renewal::Invalidated,
        "an activation admitted under one binding does not survive a refresh to another"
    );
    assert!(grant.invalidated.is_cancelled());
    assert!(gate.admit_compressed_activation().is_ok());
}

#[test]
fn the_manifest_carries_the_flag_and_the_optional_vector_limits_without_making_them_hooks() {
    let identity = identity("k", 8);
    let invalidation = json!({
        "schema_version": identity.schema_version,
        "tokenizer_fingerprint": identity.tokenizer_fingerprint,
        "analysis_identity": identity.analysis_identity,
        "embedding_model": identity.embedding_model,
        "projection_policy_version": identity.projection_policy_version,
        "identity_contract_version": identity.identity_contract_version,
        "limit_manifest_protocol_version": identity.limit_manifest_protocol_version,
        "vector_dimension": identity.vector_dimension,
        "generation_epoch": identity.generation_epoch,
    });
    let mut limits = serde_json::Map::new();
    for name in daemon::projection_gates::REQUIRED_LIMITS {
        limits.insert(name.to_owned(), json!(1));
    }
    let mut hooks = serde_json::Map::new();
    for hook in ProjectionHook::ALL {
        hooks.insert(hook.id().to_owned(), json!({ "enabled": true }));
    }
    let manifest = |limits: &serde_json::Map<String, serde_json::Value>,
                    hooks: &serde_json::Map<String, serde_json::Value>| {
        RuntimeManifest::parse(&json!({
            "protocol_version": identity.limit_manifest_protocol_version,
            "invalidation_identity": invalidation,
            "limits": limits,
            "hooks": hooks,
        }))
    };
    let plain = manifest(&limits, &hooks).unwrap();
    assert!(!plain.compressed_activation, "absent is disabled");
    assert!(
        VECTOR_LIMITS
            .iter()
            .all(|name| !plain.limits.contains_key(*name))
    );
    assert_eq!(plain.enabled.len(), ProjectionHook::ALL.len());

    hooks.insert(
        COMPRESSED_ACTIVATION_ID.to_owned(),
        json!({ "enabled": true }),
    );
    for name in VECTOR_LIMITS {
        limits.insert(name.to_owned(), json!(4096));
    }
    let flagged = manifest(&limits, &hooks).unwrap();
    assert!(flagged.compressed_activation);
    assert_eq!(flagged.limits["vector_disk_bytes"], 4096);
    assert_eq!(
        flagged.enabled.len(),
        ProjectionHook::ALL.len(),
        "the flag is not a projection hook"
    );

    limits.insert("vector_disk_bytes".to_owned(), json!("lots"));
    assert_eq!(
        manifest(&limits, &hooks).unwrap_err(),
        daemon::projection_gates::ManifestRefusal::NonNumericLimit("vector_disk_bytes".to_owned())
    );
    limits.insert("vector_disk_bytes".to_owned(), json!(4096));
    limits.insert("vector_unknown".to_owned(), json!(1));
    assert_eq!(
        manifest(&limits, &hooks).unwrap_err(),
        daemon::projection_gates::ManifestRefusal::UnknownLimit("vector_unknown".to_owned())
    );
}
