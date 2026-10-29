use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use daemon::attestation_verifier::Reason;
use daemon::attestation_verifier::classify::{
    BoundRoot, Class, Classification, DocumentKind, MAX_CITATIONS, Unverifiable,
    VERIFIER_SOURCE_KIND, VerifierInput, classify,
};
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, AnchorSpec, AnchorValue, ArtifactDeletionIdentity,
    ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactIngestRequest, ClaimFactsError,
    CommitIntent, CommitReadTarget, DecisionEventPayload, DecisionEventSpec, DecisionPayload,
    DecisionSpec, Disposition, DomainSpec, EventKind, KernelError, KernelStore, ProviderEgress,
    Sensitivity, SourceClass, SourceDescriptorPolicy, SourceDescriptorRequest, TaintClass,
};
use sha2::{Digest as _, Sha256};

const REPOSITORY: &str = "git:0000000000000000000000000000000000000001";
const PROJECT: &str = "proj-a";
const CITATION_EVENT_KIND: &str = "memory_capture_citation";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "attestation-verifier-classify-test".into(),
        operation_key: key.into(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".into(),
        cause: "proof".into(),
    }
}

fn oid(byte: u8) -> String {
    format!("{byte:02x}").repeat(20)
}

struct Evidence {
    id: String,
    digest: String,
}

#[derive(Default)]
struct Cites<'a> {
    row: Option<&'a Evidence>,
    events: Vec<&'a Evidence>,
}

struct Store {
    root: tempfile::TempDir,
    kernel: KernelStore,
}

impl Store {
    fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path()).unwrap();
        kernel
            .commit(intent("domains"), |envelope| {
                for domain in ["memory", "source"] {
                    envelope.insert_domain(DomainSpec {
                        domain_id: domain.into(),
                        object_id: format!("{domain}-domain"),
                        name: domain.into(),
                        source_kind: "test".into(),
                        source_id: domain.into(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                }
                Ok(String::new())
            })
            .unwrap();
        Self { root, kernel }
    }

    fn sqlite(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.root.path().join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .unwrap()
    }

    fn retain(&self, key: &str, text: &str) -> Evidence {
        let handle = self
            .kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("srcev:{key}"),
                object_id: format!("srcev-object:{key}"),
                object_kind: "evidence".into(),
                domain_id: "source".into(),
                source_kind: "test".into(),
                source_id: format!("source/{key}"),
                source_revision: 1,
                media_type: "text/plain".into(),
                retention_class: "canonical".into(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap();
        Evidence {
            id: handle.evidence_id,
            digest: handle.digest,
        }
    }

    fn publish(
        &self,
        key: &str,
        class: OccurrenceClass,
        identity: &[(&str, &str)],
        text: &str,
        evidence: &Evidence,
    ) -> String {
        self.kernel
            .commit(intent(&format!("descriptor-{key}")), |envelope| {
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: class.code(),
                            identity,
                            revision: "1",
                            representation: "text",
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: "source",
                        scope_id: None,
                        evidence_id: &evidence.id,
                        artifact_digest: &evidence.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Sensitive,
                        observed_at: 1,
                        expected_head: None,
                        outcome: None,
                    })
                    .map_err(|_| KernelError::InvalidInput)?;
                Ok(outcome.object_id)
            })
            .unwrap()
            .result
    }

    fn document(&self, key: &str, path: &str, text: &str) -> (Evidence, String) {
        let evidence = self.retain(key, text);
        let identity = [
            ("project_id", PROJECT),
            ("repository_id", REPOSITORY),
            ("relative_path", path),
        ];
        let object_id = self.publish(
            key,
            OccurrenceClass::RepositoryDocuments,
            &identity,
            text,
            &evidence,
        );
        (evidence, object_id)
    }

    fn message(&self, key: &str, text: &str) -> Evidence {
        let evidence = self.retain(key, text);
        let identity = [
            ("project_id", PROJECT),
            ("harness", "opencode"),
            ("session_id", "sess-01"),
            ("message_id", key),
            ("block_index", "0"),
        ];
        self.publish(key, OccurrenceClass::Messages, &identity, text, &evidence);
        evidence
    }

    fn decision(
        &self,
        name: &str,
        source_kind: &str,
        anchor: Option<AnchorValue>,
        cites: &Cites<'_>,
    ) -> String {
        let object_id = format!("mem_{name}");
        let decision_id = format!("dec_{name}");
        self.kernel
            .commit(intent(&format!("decision-{name}")), |envelope| {
                let anchor_id = match anchor {
                    Some(value) => {
                        let anchor_id = format!("anchor_{name}");
                        envelope.insert_anchor(AnchorSpec {
                            anchor_id: anchor_id.clone(),
                            object_id: format!("anchorobj_{name}"),
                            domain_id: "memory".into(),
                            source_kind: source_kind.into(),
                            source_id: "memory_capture".into(),
                            source_revision: 1,
                            sensitivity: Sensitivity::Normal,
                            value,
                        })?;
                        Some(anchor_id)
                    }
                    None => None,
                };
                envelope.insert_decision(DecisionSpec {
                    decision_id: decision_id.clone(),
                    object_id: object_id.clone(),
                    domain_id: "memory".into(),
                    proposition_id: None,
                    scope_id: None,
                    anchor_id,
                    evidence_id: cites.row.map(|evidence| evidence.id.clone()),
                    decision_kind: "ARCHITECTURE".into(),
                    payload: DecisionPayload {
                        summary: format!("decision {name}"),
                        rationale: "{}".into(),
                    },
                    source_kind: source_kind.into(),
                    source_id: "memory_capture".into(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                for evidence in &cites.events {
                    envelope.append_decision_event(
                        &decision_id,
                        DecisionEventSpec {
                            event_kind: CITATION_EVENT_KIND.into(),
                            payload: DecisionEventPayload {
                                summary: "cites the captured source message block".into(),
                            },
                            evidence_id: Some(evidence.id.clone()),
                            recorded_at: 1,
                        },
                    )?;
                }
                admission(envelope, &object_id, EventKind::Other)?;
                Ok(String::new())
            })
            .unwrap();
        object_id
    }

    fn target(&self) -> CommitReadTarget {
        self.kernel.capture_commit_read_target().unwrap()
    }

    fn classify_at(
        &self,
        object_id: &str,
        target: CommitReadTarget,
        roots: &[BoundRoot<'_>],
    ) -> Classification {
        classify(
            &self.kernel,
            object_id,
            target,
            roots,
            &EvalBudget::unbounded(),
        )
        .unwrap()
    }

    fn class(&self, object_id: &str) -> Class {
        self.classify_at(object_id, self.target(), &[]).class
    }
}

fn admission(
    envelope: &mut kernel::Envelope<'_>,
    object_id: &str,
    kind: EventKind,
) -> Result<(), KernelError> {
    envelope.record_admission(AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.into()),
        source_class: Some(SourceClass::ModelInference),
        taint_class: Some(TaintClass::AssistantInference),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".into(),
        },
    })?;
    Ok(())
}

fn reachable_from(byte: u8) -> Option<AnchorValue> {
    Some(AnchorValue::ReachableFrom {
        oid: oid(byte),
        captures: Vec::new(),
    })
}

fn unverifiable(reason: Unverifiable) -> Class {
    Class::Unverifiable(reason)
}

fn verifiable(class: Class) -> VerifierInput {
    match class {
        Class::Verifiable(input) => *input,
        other => panic!("expected a verifiable decision, found {other:?}"),
    }
}

fn digest_bytes(hex: &str) -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).unwrap();
    }
    bytes
}

fn events<'a>(evidence: &[&'a Evidence]) -> Cites<'a> {
    Cites {
        row: None,
        events: evidence.to_vec(),
    }
}

#[test]
fn each_unverifiable_decision_classifies_under_exactly_one_reason_and_nothing_is_written() {
    let store = Store::open();
    let chat = store.message("chat", "Use port 4321 for staging.");
    let (document, _) = store.document("doc", "ARCHITECTURE.md", "# Architecture\n");
    let (deleted, _) = store.document("deleted", "docs/deleted.md", "# Deleted\n");
    let (retired, retired_descriptor) = store.document("retired", "docs/retired.md", "# Old\n");
    let cases = [
        (
            store.decision(
                "messages_only",
                "assistant",
                reachable_from(1),
                &events(&[&chat]),
            ),
            Unverifiable::CitationNotDocument,
        ),
        (
            store.decision("unanchored", "assistant", None, &events(&[&document])),
            Unverifiable::NoAnchor,
        ),
        (
            store.decision(
                "exact",
                "assistant",
                Some(AnchorValue::Exact {
                    value: "token".into(),
                }),
                &events(&[&document]),
            ),
            Unverifiable::AnchorNotGit,
        ),
        (
            store.decision(
                "between",
                "assistant",
                Some(AnchorValue::ReachableBetween {
                    start_oid: oid(2),
                    end_oid: oid(3),
                    captures: Vec::new(),
                }),
                &events(&[&document]),
            ),
            Unverifiable::AnchorNotGit,
        ),
        (
            store.decision(
                "deployment",
                "assistant",
                Some(AnchorValue::DeploymentRevision {
                    revision: "r1".into(),
                }),
                &events(&[&document]),
            ),
            Unverifiable::AnchorNotGit,
        ),
        (
            store.decision(
                "evidence_retired",
                "assistant",
                reachable_from(4),
                &events(&[&chat, &deleted]),
            ),
            Unverifiable::EvidenceRetired,
        ),
        (
            store.decision(
                "descriptor_retired",
                "assistant",
                reachable_from(5),
                &events(&[&retired]),
            ),
            Unverifiable::DescriptorRetired,
        ),
        (
            store.decision("uncited", "assistant", reachable_from(6), &Cites::default()),
            Unverifiable::NoCitation,
        ),
    ];
    let control = store.decision(
        "control",
        "assistant",
        reachable_from(7),
        &events(&[&document]),
    );
    store
        .kernel
        .delete_artifact(ArtifactDeletionRequest {
            intent: intent("delete-deleted"),
            identity: ArtifactDeletionIdentity::Digest(deleted.digest.clone()),
            kind: ArtifactDeletionKind::Delete,
            operator_id: None,
            target_locator: None,
            reason: None,
            deleted_at: 1,
        })
        .unwrap();
    store
        .kernel
        .commit(intent("retire-descriptor"), |envelope| {
            envelope.retire_observation(&retired_descriptor)?;
            Ok(String::new())
        })
        .unwrap();

    let tip = store.kernel.tip().unwrap();
    for (object_id, reason) in &cases {
        assert_eq!(store.class(object_id), unverifiable(*reason), "{object_id}");
    }
    assert!(
        matches!(store.class(&control), Class::Verifiable(_)),
        "the shared document citation is verifiable behind a git anchor"
    );
    assert_eq!(
        store.kernel.tip().unwrap(),
        tip,
        "classification commits nothing"
    );
    let names = [
        Unverifiable::NoAnchor,
        Unverifiable::AnchorNotGit,
        Unverifiable::NoCitation,
        Unverifiable::CitationNotDocument,
        Unverifiable::EvidenceRetired,
        Unverifiable::DescriptorRetired,
        Unverifiable::DigestMissing,
        Unverifiable::SelfSource,
    ]
    .map(Unverifiable::as_str);
    assert_eq!(
        names,
        [
            "no_anchor",
            "anchor_not_git",
            "no_citation",
            "citation_not_document",
            "evidence_retired",
            "descriptor_retired",
            "digest_missing",
            "self_source",
        ]
    );
}

#[test]
fn a_descriptor_without_a_digest_is_digest_missing() {
    let store = Store::open();
    let (bare, _) = store.document("bare", "docs/bare.md", "# Bare\n");
    let chat = store.message("chat", "chat text");
    let object_id = store.decision(
        "bare",
        "assistant",
        reachable_from(1),
        &events(&[&chat, &bare]),
    );
    let connection = store.sqlite();
    let payload: Vec<u8> = connection
        .query_row(
            "SELECT observation_payload FROM observations WHERE evidence_id=?1",
            [&bare.id],
            |row| row.get(0),
        )
        .unwrap();
    let mut stored: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    let mut detail: serde_json::Value =
        serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
    detail["artifact_digest"] = "".into();
    stored["detail"] = detail.to_string().into();
    connection
        .execute(
            "UPDATE observations SET observation_payload=?1 WHERE evidence_id=?2",
            rusqlite::params![serde_json::to_vec(&stored).unwrap(), bare.id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE evidence_meta SET artifact_digest='' WHERE evidence_id=?1",
            [&bare.id],
        )
        .unwrap();
    assert_eq!(
        store.class(&object_id),
        unverifiable(Unverifiable::DigestMissing)
    );
}

#[test]
fn a_verifier_decision_is_self_source_before_any_anchor_or_citation_read() {
    let store = Store::open();
    let verifier = store.decision(
        "attestation",
        VERIFIER_SOURCE_KIND,
        reachable_from(1),
        &Cites::default(),
    );
    let assistant = store.decision("model", "assistant", reachable_from(2), &Cites::default());
    assert_eq!(
        store.class(&assistant),
        unverifiable(Unverifiable::NoCitation)
    );
    store
        .sqlite()
        .execute(
            "UPDATE anchors SET reachable_from_oid='not-an-oid'
             WHERE anchor_id IN ('anchor_attestation','anchor_model')",
            [],
        )
        .unwrap();
    assert!(matches!(
        classify(
            &store.kernel,
            &assistant,
            store.target(),
            &[],
            &EvalBudget::unbounded()
        ),
        Err(ClaimFactsError::Kernel(KernelError::CorruptCanonicalRow))
    ));
    assert_eq!(
        store.class(&verifier),
        unverifiable(Unverifiable::SelfSource)
    );
}

#[test]
fn a_decision_outside_the_input_set_counts_under_no_reason() {
    let store = Store::open();
    let (document, _) = store.document("doc", "ARCHITECTURE.md", "# Architecture\n");
    let stale = store.decision(
        "stale",
        "assistant",
        reachable_from(1),
        &events(&[&document]),
    );
    let retired = store.decision(
        "retired",
        "assistant",
        reachable_from(2),
        &events(&[&document]),
    );
    let before = store.target();
    store
        .kernel
        .commit(intent("stale"), |envelope| {
            admission(envelope, &stale, EventKind::MarkStale)?;
            envelope.retire_decision(&retired)?;
            Ok(String::new())
        })
        .unwrap();
    for object_id in [
        stale.as_str(),
        retired.as_str(),
        "mem_never",
        "memory-domain",
    ] {
        assert_eq!(store.class(object_id), Class::NotInput, "{object_id}");
    }
    for object_id in [&stale, &retired] {
        assert!(
            matches!(
                store.classify_at(object_id, before, &[]).class,
                Class::Verifiable(_)
            ),
            "{object_id} was input at the earlier sequence"
        );
    }
    let cancelled = EvalBudget::new(None, Arc::new(true.into()));
    assert!(matches!(
        classify(&store.kernel, &stale, store.target(), &[], &cancelled),
        Err(ClaimFactsError::Kernel(KernelError::Deadline))
    ));
}

#[test]
fn a_decision_citing_a_message_and_a_document_is_verifiable_through_the_document() {
    let store = Store::open();
    let text = "# Architecture\n\nThe daemon owns the store.\n";
    let chat = store.message("chat", "The daemon owns the store.");
    let (document, descriptor) = store.document("doc", "ARCHITECTURE.md", text);
    let object_id = store.decision(
        "both",
        "assistant",
        reachable_from(9),
        &Cites {
            row: Some(&chat),
            events: vec![&chat, &document],
        },
    );
    let target = store.target();
    let classification = store.classify_at(&object_id, target, &[]);
    assert_eq!(classification.commit_seq, target.through_commit);
    let input = verifiable(classification.class);
    assert_eq!(input.claim.relative_path(), "ARCHITECTURE.md");
    assert_eq!(input.claim.digest(), &digest_bytes(&document.digest));
    assert_eq!(
        input.claim.digest(),
        &<[u8; 32]>::from(Sha256::digest(text))
    );
    assert_eq!(input.claim.byte_length(), text.len() as u64);
    assert_eq!(input.claim.oid(), oid(9));
    assert_eq!(input.evidence_id, document.id);
    assert_eq!(input.descriptor_object_id, descriptor);
    assert_eq!(input.descriptor_revision, "1");
    assert!(descriptor.starts_with(&format!("srcdesc:{}:", input.lineage_id)));
    assert_eq!(input.repository_id, REPOSITORY);
    assert_eq!(input.root, Err(Reason::RepositoryUnbound));
    assert_eq!(input.document_kind, DocumentKind::Code);
    let decision = &input.decision;
    assert_eq!(decision.object_id, object_id);
    assert_eq!(decision.decision_id, "dec_both");
    assert_eq!(decision.source_kind, "assistant");
    assert_eq!(decision.source_id, "memory_capture");
    assert_eq!(decision.source_revision, 1);
    assert!(decision.created_commit_seq <= target.through_commit);
    assert_eq!(decision.anchor_id.as_deref(), Some("anchor_both"));
    assert_eq!(decision.evidence_id.as_deref(), Some(chat.id.as_str()));
    assert_eq!(decision.disposition, Disposition::Active);
    assert_eq!(decision.source_class, SourceClass::ModelInference);
    assert_eq!(decision.taint_class, TaintClass::AssistantInference);
    assert_eq!(decision.sensitivity, Sensitivity::Sensitive);
    let rendered = format!("{input:?}");
    assert!(!rendered.contains("The daemon owns the store."));
    assert!(MAX_CITATIONS.get() >= 2);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn classification_at_one_sequence_ignores_the_worktree_head_and_later_commits() {
    let store = Store::open();
    let text = "# Architecture\n";
    let (document, descriptor) = store.document("doc", "ARCHITECTURE.md", text);
    let checkout = tempfile::tempdir().unwrap();
    git(
        checkout.path(),
        &["init", "--quiet", "--initial-branch=main"],
    );
    std::fs::write(checkout.path().join("ARCHITECTURE.md"), text).unwrap();
    git(checkout.path(), &["add", "ARCHITECTURE.md"]);
    git(checkout.path(), &["commit", "--quiet", "-m", "doc"]);
    let head = git(checkout.path(), &["rev-parse", "HEAD"]);
    let object_id = store.decision(
        "pinned",
        "assistant",
        Some(AnchorValue::ReachableFrom {
            oid: head,
            captures: Vec::new(),
        }),
        &events(&[&document]),
    );
    let roots = [BoundRoot {
        root: checkout.path(),
        repository_id: REPOSITORY,
    }];
    let target = store.target();
    let first = store.classify_at(&object_id, target, &roots);

    std::fs::write(checkout.path().join("ARCHITECTURE.md"), "# Edited\n").unwrap();
    git(checkout.path(), &["commit", "--quiet", "-am", "edit"]);
    std::fs::write(checkout.path().join("ARCHITECTURE.md"), "# Dirty\n").unwrap();
    store
        .kernel
        .commit(intent("retire-later"), |envelope| {
            envelope.retire_observation(&descriptor)?;
            Ok(String::new())
        })
        .unwrap();
    assert!(store.kernel.tip().unwrap() > target.through_commit);
    let second = store.classify_at(&object_id, target, &roots);
    assert_eq!(first, second);
    assert_eq!(
        verifiable(second.class).root,
        Ok(checkout.path().to_path_buf())
    );
    assert_eq!(
        store.class(&object_id),
        unverifiable(Unverifiable::DescriptorRetired),
        "the later commit is visible at the tip"
    );

    let source = include_str!("../src/attestation_verifier/classify.rs");
    for api in [
        "gix",
        "git_sources",
        "verify(",
        "std::fs",
        "Instant::now",
        "SystemTime",
        "std::process",
        "std::net",
    ] {
        assert!(!source.contains(api), "the classifier names {api}");
    }
}

#[test]
fn the_repository_binds_to_the_first_matching_root_in_bound_order() {
    let store = Store::open();
    let (document, _) = store.document("doc", "config/app.toml", "port = 4321\n");
    let object_id = store.decision(
        "config",
        "assistant",
        reachable_from(1),
        &events(&[&document]),
    );
    let (a, b, c) = (
        Path::new("/bound/a"),
        Path::new("/bound/b"),
        Path::new("/bound/c"),
    );
    let bound = |repository_id, root| BoundRoot {
        root,
        repository_id,
    };
    let input = |roots: &[BoundRoot<'_>]| {
        verifiable(store.classify_at(&object_id, store.target(), roots).class)
    };
    let matched = input(&[
        bound("git:other", a),
        bound(REPOSITORY, b),
        bound(REPOSITORY, c),
    ]);
    assert_eq!(matched.root, Ok(b.to_path_buf()));
    assert_eq!(matched.document_kind, DocumentKind::Config);
    assert_eq!(
        input(&[bound(REPOSITORY, c), bound(REPOSITORY, b)]).root,
        Ok(c.to_path_buf())
    );
    assert_eq!(
        input(&[bound("git:other", a)]).root,
        Err(Reason::RepositoryUnbound)
    );
}

#[test]
fn the_document_kind_path_rule_is_a_closed_table() {
    let config = [
        "Cargo.toml",
        "package.json",
        "tsconfig.json",
        "config/app.yaml",
        "deploy/ci.yml",
        "setup.cfg",
        "tox.ini",
        "nginx/site.conf",
        ".env",
        "prod.env",
        ".editorconfig",
        "nested/dir/.editorconfig",
    ];
    let code = [
        "ARCHITECTURE.md",
        "STRUCTURE.md",
        "README",
        "src/lib.rs",
        "Makefile",
        "docs/toml",
        "toml.md",
        "Cargo.TOML",
        "env",
        "config.json.md",
    ];
    for path in config {
        let kind = DocumentKind::of(path);
        assert_eq!(kind, DocumentKind::Config, "{path}");
        assert_eq!(kind.taint_class(), TaintClass::CurrentConfig);
        assert_eq!(kind.witness_kind(), "config_present");
    }
    for path in code {
        let kind = DocumentKind::of(path);
        assert_eq!(kind, DocumentKind::Code, "{path}");
        assert_eq!(kind.taint_class(), TaintClass::CurrentCode);
        assert_eq!(kind.witness_kind(), "code_present");
    }
}
