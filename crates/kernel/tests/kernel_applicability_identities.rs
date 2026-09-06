//! Pins the applicability identities that are written into durable rows, so a
//! rename is observed here rather than only through code that compares a
//! constant with itself.

use kernel::applicability::{
    OBJECT_APPLICABILITY_SCHEMA, OBSERVATION_APPLICABILITY_SCHEMA, ObjectApplicabilitySpec,
    PATCH_ID_ALGORITHM, PayloadDecode, checkout_identity_digest,
};

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
