//! The step decoder against a broker with one issued alias: unknown fields, unknown variants, out-of-range values, aliases the broker never issued, malformed ranges, oversized batches, and proposals the Kernel's own rules would refuse are all rejected before any effect, and each rejection is the code the model is shown.

use daemon::curator::broker::{
    EvidenceBroker, QuestionTemplate, ReferenceExpectation, RefusalCode, RunBinding,
};
use daemon::curator::model_response::MAX_ASSISTANT_TEXT_BYTES;
use daemon::curator::steps::{Operation, Step};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    CanonicalTarget, CuratorHoldBinding, MAX_REVIEW_TEXT_BYTES, ManifestReference,
    PolicyDependencies, ProjectScope, ProposalAction, ProposalTarget, ReviewPayload,
    ReviewProposal, ReviewQuestionTemplate, Uncertainty,
};

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A broker that has issued exactly `ref-1`; decoding touches no store, so none is opened.
fn broker() -> EvidenceBroker {
    let mut broker = EvidenceBroker::new(
        RunBinding {
            project: ProjectScope::new(PROJECT).unwrap(),
            hold: CuratorHoldBinding {
                project_digest: PROJECT.to_string(),
                kernel_incarnation: "k".repeat(32),
                memstore_incarnation: "m".repeat(32),
                subject: "job-1".to_string(),
                generation: 1,
            },
            hold_id: "hold-1".to_string(),
            destination: kernel::ArtifactDestination::Remote,
        },
        QuestionTemplate::ExtractedFacts,
    );
    broker.aliases.issue(ReferenceExpectation::NativeSource {
        object_id: "descriptor-1".to_string(),
        class: OccurrenceClass::GitCommits,
        source_revision: 1,
        artifact_digest: "d".repeat(64),
        evidence_id: "srcev:1".to_string(),
        occurrence_tuple: vec![1, 2, 3],
    });
    broker
}

#[test]
fn steps_are_rejected_before_any_effect() {
    let broker = broker();
    let rows: &[(&str, bool, Result<(), RefusalCode>)] = &[
        ("not json", true, Err(RefusalCode::Undecodable)),
        (
            r#"{"v":2,"step":{"kind":"abstain","reason":"x"}}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"abstain","reason":"x"},"extra":1}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"abstain","reason":"x","why":"y"}}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"dance"}}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[]}}"#,
            true,
            Err(RefusalCode::BatchLimit),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"find_related"},{"op":"find_related"},{"op":"find_related"},{"op":"find_related"},{"op":"find_related"},{"op":"find_related"},{"op":"find_related"},{"op":"find_related"},{"op":"find_related"}]}}"#,
            true,
            Err(RefusalCode::BatchLimit),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"ref-9"}]}}"#,
            true,
            Err(RefusalCode::UnknownAlias),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"ref-1","range":{"start":5,"end":5}}]}}"#,
            true,
            Err(RefusalCode::InvalidRange),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"ref-1","range":{"start":0,"end":5,"step":1}}]}}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"search_project","by":"content","literal":""}]}}"#,
            true,
            Err(RefusalCode::TooLarge),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"search_project","by":"regex","literal":"x"}]}}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_project","path":""}]}}"#,
            true,
            Err(RefusalCode::TooLarge),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_project","path":"../etc/passwd"}]}}"#,
            true,
            Ok(()),
        ),
        (
            r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_reference","alias":"ref-1"}]}}"#,
            true,
            Ok(()),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"retain","support":[{"alias":"ref-1"}],"uncertainty":"low"}}"#,
            true,
            Ok(()),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"retain","new_text":"x","support":[],"uncertainty":"low"}}"#,
            true,
            Err(RefusalCode::Unsupported),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"create","new_text":"x","support":[],"uncertainty":"low"}}"#,
            true,
            Err(RefusalCode::Unsupported),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"create","new_text":"x","support":[],"uncertainty":"low"}}"#,
            false,
            Ok(()),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"revise","support":[],"uncertainty":"low"}}"#,
            true,
            Err(RefusalCode::Unsupported),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"retire","support":[{"alias":"ref-2"}],"uncertainty":"low"}}"#,
            true,
            Err(RefusalCode::UnknownAlias),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"retire","support":[{"alias":"ref-1","range":{"start":3,"end":1}}],"uncertainty":"low"}}"#,
            true,
            Err(RefusalCode::InvalidRange),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"retain","support":[],"limitations":["","x"],"uncertainty":"low"}}"#,
            true,
            Err(RefusalCode::TooLarge),
        ),
        (
            r#"{"v":1,"step":{"kind":"propose","action":"retain","support":[],"uncertainty":"certain"}}"#,
            true,
            Err(RefusalCode::Undecodable),
        ),
        (
            r#"{"v":1,"step":{"kind":"abstain","reason":"the evidence does not warrant a conclusion"}}"#,
            true,
            Ok(()),
        ),
    ];
    for (text, targets_memory, expected) in rows {
        let outcome = Step::parse(text, &broker, *targets_memory).map(|_| ());
        assert_eq!(outcome, *expected, "{text}");
    }
    // A traversal path decodes; confinement is the project-text reader's to refuse, on the read itself.
    let Step::ReadBatch { operations } = Step::parse(
        r#"{"v":1,"step":{"kind":"read_batch","operations":[{"op":"read_project","path":"../etc/passwd"}]}}"#,
        &broker,
        true,
    )
    .unwrap() else {
        panic!("a read batch")
    };
    assert!(
        matches!(&operations[0], Operation::ReadProject { path, .. } if path == "../etc/passwd")
    );
    let oversized = format!(
        r#"{{"v":1,"step":{{"kind":"propose","action":"revise","new_text":"{}","support":[],"uncertainty":"low"}}}}"#,
        "x".repeat(MAX_REVIEW_TEXT_BYTES + 1)
    );
    assert_eq!(
        Step::parse(&oversized, &broker, true).map(|_| ()),
        Err(RefusalCode::TooLarge)
    );
}

/// The fixture uses the smallest valid values for host-populated fields.
fn staged(new_text: &str, limitations: &[&str]) -> ReviewPayload {
    ReviewPayload::Proposal(Box::new(ReviewProposal {
        action: ProposalAction::Revise,
        target: ProposalTarget::Memory(CanonicalTarget {
            object_id: "m".to_string(),
            source_revision: 0,
            known_as_of: 0,
            commit_token: 0,
        }),
        new_text: Some(new_text.to_string()),
        support: vec![],
        contradictions: vec![],
        limitations: limitations.iter().map(|text| text.to_string()).collect(),
        uncertainty: Uncertainty::Low,
        manifest: ManifestReference {
            manifest_id: "manifest:x:1".to_string(),
            digest: "0".repeat(64),
        },
        policy_dependencies: PolicyDependencies {
            question_template: ReviewQuestionTemplate::ExtractedFacts,
            disclosed_inputs: vec![],
            uncited_disclosed_inputs: vec![],
            ancestry: vec![],
        },
    }))
}

fn propose(new_text: &str, limitations: &[&str]) -> String {
    let limitations: Vec<String> = limitations
        .iter()
        .map(|text| format!("\"{text}\""))
        .collect();
    format!(
        r#"{{"v":1,"step":{{"kind":"propose","action":"revise","new_text":"{new_text}","support":[],"limitations":[{}],"uncertainty":"low"}}}}"#,
        limitations.join(",")
    )
}

/// A reply that fits the assistant-text allowance, with every field inside its own Kernel bound, can still exceed the Kernel's payload bound once the host's fields are added. The decoder bounds the model's text in aggregate so that refusal is shown to the model instead of failing at staging.
#[test]
fn proposal_text_is_bounded_in_aggregate() {
    let broker = broker();
    let new_text = "x".repeat(MAX_REVIEW_TEXT_BYTES);
    let envelope = propose("", &[""]).len();
    let limitation = "y".repeat(MAX_ASSISTANT_TEXT_BYTES - envelope - new_text.len());
    assert!(limitation.len() <= MAX_REVIEW_TEXT_BYTES);
    let reply = propose(&new_text, &[&limitation]);
    assert_eq!(reply.len(), MAX_ASSISTANT_TEXT_BYTES);
    assert!(staged(&new_text, &[&limitation]).encode().is_err());
    assert_eq!(
        Step::parse(&reply, &broker, true).map(|_| ()),
        Err(RefusalCode::TooLarge)
    );
    // Text that together fills the Kernel's text bound exactly is accepted and stages.
    let half = "z".repeat(MAX_REVIEW_TEXT_BYTES / 2);
    assert!(staged(&half, &[&half]).encode().is_ok());
    assert!(Step::parse(&propose(&half, &[&half]), &broker, true).is_ok());
    let over = format!("{half}z");
    assert_eq!(
        Step::parse(&propose(&half, &[&over]), &broker, true).map(|_| ()),
        Err(RefusalCode::TooLarge)
    );
}
