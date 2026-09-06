//! Pins the applicability identities that are written into durable rows, so a
//! rename is observed here rather than only through code that compares a
//! constant with itself.
//!
//! A repair operation key hashes the checkout identity, which is derived from
//! the git directory path, so the key itself cannot be pinned to a literal.
//! The derivation is pinned instead: the key is recomputed here from the
//! snapshot's and object's public accessors.

#[path = "support/git_fixtures.rs"]
mod git_fixtures;

use git_fixtures::{commit_snapshot, init_repo, materialize, set_head_detached};
use kernel::applicability::{
    ApplicabilityCandidate, ApplicabilityEngine, ApplicabilityState, CheckSpec, EvalBudget,
    OBJECT_APPLICABILITY_SCHEMA, OBSERVATION_APPLICABILITY_SCHEMA, OBSERVATION_KIND_CURRENT,
    OBSERVATION_KIND_DIRTY_TREE_UNCERTAIN, OBSERVATION_KIND_HISTORICAL,
    OBSERVATION_KIND_LIFECYCLE_INVALIDATED, OBSERVATION_KIND_OUT_OF_SCOPE, OBSERVATION_KIND_STALE,
    OBSERVATION_KIND_UNCERTAIN, ObjectApplicabilitySpec, PATCH_ID_ALGORITHM, PayloadDecode,
    RepairIntent, checkout_identity_digest, snapshot_checkout,
};
use kernel::{QueryContext, ScopeMatchContext};
use sha2::{Digest, Sha256};

#[test]
fn stored_identity_literals_are_pinned() {
    assert_eq!(
        OBJECT_APPLICABILITY_SCHEMA,
        "eidnara.applicability.object.v1"
    );
    assert_eq!(
        OBSERVATION_APPLICABILITY_SCHEMA,
        "eidnara.applicability.observation.v2"
    );
    assert_eq!(PATCH_ID_ALGORITHM, "eidnara-patch-id-v4");
}

/// `printf 'eidnara-applicability-checkout-v1\0/repo' | sha256sum`
#[test]
fn checkout_identity_digest_matches_a_recorded_value() {
    assert_eq!(
        checkout_identity_digest("/repo"),
        "854a4c88aa7c7307d31d06ad213fac4cb3ff1a4e1e34e99b3283511c7594ee61"
    );
}

/// A payload written under the recorded schema id decodes; one under the
/// predecessor id is undecodable rather than silently accepted.
#[test]
fn object_payload_round_trips_under_the_recorded_schema_id() {
    let current =
        br#"{"schema":"eidnara.applicability.object.v1","affected_paths":["src/lib.rs"]}"#;
    match ObjectApplicabilitySpec::decode(Some(current)) {
        PayloadDecode::Present(spec) => {
            assert_eq!(spec.schema, OBJECT_APPLICABILITY_SCHEMA);
            assert_eq!(spec.affected_paths, ["src/lib.rs"]);
        }
        other => panic!("current schema must decode: {other:?}"),
    }
    let other_schema = br#"{"schema":"eidnara.applicability.object.v0","affected_paths":[]}"#;
    assert!(matches!(
        ObjectApplicabilitySpec::decode(Some(other_schema)),
        PayloadDecode::Undecodable(_)
    ));
}

/// Every state maps to its own stored observation kind. The literals are
/// spelled out so a renamed constant fails here rather than only where the
/// constant is compared with itself.
#[test]
fn observation_kind_literals_are_pinned_and_distinct() {
    let table = [
        (
            ApplicabilityState::Current,
            OBSERVATION_KIND_CURRENT,
            "applicability.current",
        ),
        (
            ApplicabilityState::Historical,
            OBSERVATION_KIND_HISTORICAL,
            "applicability.historical",
        ),
        (
            ApplicabilityState::OutOfScope,
            OBSERVATION_KIND_OUT_OF_SCOPE,
            "applicability.out_of_scope",
        ),
        (
            ApplicabilityState::Uncertain,
            OBSERVATION_KIND_UNCERTAIN,
            "applicability.uncertain",
        ),
        (
            ApplicabilityState::DirtyTreeUncertain,
            OBSERVATION_KIND_DIRTY_TREE_UNCERTAIN,
            "applicability.dirty_tree_uncertain",
        ),
        (
            ApplicabilityState::Stale,
            OBSERVATION_KIND_STALE,
            "applicability.stale",
        ),
        (
            ApplicabilityState::LifecycleInvalidated,
            OBSERVATION_KIND_LIFECYCLE_INVALIDATED,
            "applicability.lifecycle_invalidated",
        ),
    ];
    for (state, constant, literal) in table {
        assert_eq!(constant, literal, "{state:?}");
        assert_eq!(state.observation_kind(), literal, "{state:?}");
    }
    let mut kinds: Vec<&str> = table.iter().map(|(_, _, literal)| *literal).collect();
    kinds.sort_unstable();
    let total = kinds.len();
    kinds.dedup();
    assert_eq!(kinds.len(), total, "observation kinds collide");
}

/// The repair operation key is `eidnara-applicability-repair-v1\0` followed
/// by nine NUL-terminated parts: repair generation, object id, object
/// revision, checkout identity, HEAD, dirty fingerprint, observation kind,
/// canonical JSON of the failed check, and the patch-ID algorithm.
#[test]
fn repair_operation_key_derivation_is_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = init_repo(dir.path());
    let tip = commit_snapshot(
        &fixture.repo,
        "main",
        &[],
        &[("src/lib.rs", "pub fn a() {}\n")],
        "seed",
        1,
    );
    set_head_detached(&fixture.repo, tip);
    materialize(&fixture.repo, tip);

    let check = CheckSpec::FileExists {
        path: "src/removed.rs".to_string(),
    };
    let candidates = [ApplicabilityCandidate {
        object_id: "target-object".to_string(),
        object_revision: 7,
        payload: Some(ObjectApplicabilitySpec::new(vec![], vec![check.clone()]).encode()),
        ..ApplicabilityCandidate::default()
    }];
    let snapshot = snapshot_checkout(dir.path(), &EvalBudget::unbounded()).unwrap();
    let batch = ApplicabilityEngine::new().evaluate_batch(
        &snapshot,
        &QueryContext::default(),
        &ScopeMatchContext::new(),
        &candidates,
        &EvalBudget::unbounded(),
    );
    let object = &batch.objects[0];
    assert_eq!(object.state, ApplicabilityState::Stale);
    assert_eq!(object.failed_check.as_ref().unwrap().check, check);
    let intent = RepairIntent::for_classification(&snapshot, object, None, "test", 42).unwrap();

    let mut key = Sha256::new();
    key.update(b"eidnara-applicability-repair-v1\0");
    for part in [
        // No durable block: generation zero.
        "0",
        "target-object",
        "7",
        snapshot.identity(),
        snapshot.head(),
        snapshot.dirty_fingerprint(),
        "applicability.stale",
        r#"{"kind":"file_exists","path":"src/removed.rs"}"#,
        "eidnara-patch-id-v4",
    ] {
        key.update(part.as_bytes());
        key.update(b"\0");
    }
    assert_eq!(intent.operation_key(), format!("{:x}", key.finalize()));
}
