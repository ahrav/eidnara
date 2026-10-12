#![cfg(feature = "test-support")]
//! Memory-domain reads over a store whose other domains and approved dependents outnumber its memory decisions.

use std::time::Instant;

use kernel::{
    AdmissionDomainSpec, AdmissionEvent, AdmissionRequest, ArtifactIngestRequest, CommitIntent,
    DecisionPayload, DecisionSpec, DomainSpec, EventKind, KernelStore, ProviderEgress,
    RepositoryProvenance, Sensitivity, SourceClass, StagingCandidateSpec, Surface, TaintClass,
    VisibleAsOf,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

const MEMORY: &str = "memory";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-canonical-read-test".to_string(),
        operation_key: key.to_string(),
        request_digest: "a".repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn domain(domain_id: &str) -> DomainSpec {
    DomainSpec {
        domain_id: domain_id.to_string(),
        object_id: format!("{domain_id}-object"),
        name: domain_id.to_string(),
        source_kind: "fixture".to_string(),
        source_id: domain_id.to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn decision(object: &str, domain_id: &str, kind: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{object}"),
        object_id: object.to_string(),
        domain_id: domain_id.to_string(),
        proposition_id: None,
        scope_id: None,
        anchor_id: None,
        evidence_id: None,
        decision_kind: kind.to_string(),
        payload: DecisionPayload {
            summary: format!("summary {object}"),
            rationale: format!("rationale {object}"),
        },
        source_kind: "repo".to_string(),
        source_id: format!("src/{object}"),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn explicit(object: &str) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(SourceClass::ExplicitUser),
        taint_class: Some(TaintClass::UserExplicit),
        event: AdmissionEvent {
            kind: EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".to_string(),
        },
    }
}

/// The `approval` fixture has standing as an admitted, active, approved decision.
fn seed_approval(root: &std::path::Path) {
    let store = KernelStore::open(root).unwrap();
    store
        .commit(intent("approval-domain"), |envelope| {
            envelope.insert_domain(domain("approval-domain"))?;
            Ok(String::new())
        })
        .unwrap();
    drop(store);
    Connection::open(root.join("kernel.sqlite"))
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys=ON;
             INSERT INTO object_registry(
                 object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                 created_commit_seq,sensitivity_class
             ) VALUES (
                 'approval','decision','approval-domain','fixture','approval',1,1,'normal'
             );
             INSERT INTO decisions(
                 decision_id,object_id,decision_kind,decision_payload,created_commit_seq,
                 sensitivity_class
             ) VALUES ('approval-decision','approval','adr_accepted',X'7b7d',1,'normal');
             INSERT INTO admission_decisions(
                 admission_decision_id,subject_object_id,source_kind,source_id,source_revision,
                 source_class,taint_class,event_kind,maturity,effective_maturity,disposition,visibility,
                 outcome,sensitivity_class,policy_revision,reason,elevated_support,commit_seq,
                 decided_at
             ) VALUES (
                 'approval-admission','approval','fixture','approval',1,'explicit_user',
                 'user_explicit','accepted_adr','approved','approved','active','automatic',
                 'admit','normal',1,'fixture',1,1,1
             );",
        )
        .unwrap();
}

/// Admits `count` domain candidates whose admissions cite `approval`.
fn admit_approved_dependents(store: &KernelStore, count: usize) {
    let now: i64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap();
    for n in 0..count {
        let candidate = format!("dependent-{n}");
        store
            .stage_candidate(StagingCandidateSpec {
                extraction_run_id: format!("run-{candidate}"),
                candidate_id: candidate.clone(),
                extractor: "fixture".to_string(),
                source_kind: "repo".to_string(),
                source_id: format!("source-{candidate}"),
                source_revision: 1,
                candidate_kind: "domain".to_string(),
                payload: format!("name-{candidate}"),
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
                recorded_at: now,
                lease_expires_at: now + 600_000,
            })
            .unwrap();
        let request = AdmissionRequest {
            candidate_id: Some(candidate.clone()),
            subject_object_id: None,
            source_class: Some(SourceClass::ModelInference),
            taint_class: Some(TaintClass::AssistantInference),
            event: AdmissionEvent {
                kind: EventKind::Verify,
                trigger_object_id: Some(format!("observation-{candidate}")),
                approval_object_id: Some("approval".to_string()),
                evidence_id: None,
                reason: "fixture".to_string(),
            },
        };
        store
            .commit(intent(&candidate), |envelope| {
                envelope.admit_domain_candidate(
                    request,
                    AdmissionDomainSpec {
                        domain_id: format!("domain-{candidate}"),
                        object_id: format!("object-{candidate}"),
                        name: format!("name-{candidate}"),
                    },
                )?;
                Ok(String::new())
            })
            .unwrap();
    }
}

/// `memory` admitted decisions in the memory domain, `noise` admitted decisions of another domain, and
/// `dependents` approved domain objects that cite one approval.
fn store_with(
    root: &std::path::Path,
    memory: usize,
    noise: usize,
    dependents: usize,
) -> KernelStore {
    seed_approval(root);
    let store = KernelStore::open(root).unwrap();
    store
        .commit(intent("domains"), |envelope| {
            envelope.insert_domain(domain(MEMORY))?;
            envelope.insert_domain(domain("noise"))?;
            Ok(String::new())
        })
        .unwrap();
    for (prefix, domain_id, count) in [("memory", MEMORY, memory), ("noise", "noise", noise)] {
        for chunk in (0..count).collect::<Vec<_>>().chunks(256) {
            store
                .commit(intent(&format!("{prefix}-{}", chunk[0])), |envelope| {
                    for n in chunk {
                        let object = format!("{prefix}-{n}");
                        let kind = if n % 3 == 0 {
                            "constraint"
                        } else {
                            "preference"
                        };
                        envelope.insert_decision(decision(&object, domain_id, kind))?;
                        envelope.record_admission(explicit(&object))?;
                    }
                    Ok(String::new())
                })
                .unwrap();
        }
    }
    admit_approved_dependents(&store, dependents);
    store
}

fn ids(visible: &VisibleAsOf) -> Vec<(String, String)> {
    visible
        .rows
        .iter()
        .map(|row| {
            (
                row.object.object_id.clone(),
                format!("{:?}", row.visibility),
            )
        })
        .collect()
}

/// The domain selection returns exactly the rows a full read keeps after filtering to memory-domain decisions,
/// with the same visibility.
#[test]
fn the_domain_selection_equals_the_filtered_full_read() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 40, 60, 12);
    let tip = store.tip().unwrap();
    for surface in [Surface::AutoInject, Surface::ExplicitSearch] {
        let all = store
            .visible_as_of_in_scope(surface, tip, None, None)
            .unwrap();
        let mut expected = all.clone();
        expected
            .rows
            .retain(|row| row.object.object_kind == "decision" && row.object.domain_id == MEMORY);
        let selected = store
            .visible_domain_decisions_as_of_in_scope(surface, tip, MEMORY, None)
            .unwrap();
        assert_eq!(ids(&selected), ids(&expected), "{surface:?}");
        assert_eq!(selected.rows, expected.rows, "{surface:?}");
        if surface == Surface::ExplicitSearch {
            assert_eq!(selected.rows.len(), 40);
        }
        assert_eq!(
            (selected.known_as_of, selected.tip),
            (all.known_as_of, all.tip)
        );
    }
}

fn ingest_decision_kind_artifact(store: &KernelStore, sensitivity: Sensitivity) -> String {
    let payload = b"an artifact registered as a decision".to_vec();
    let mut intent = intent("decision-kind-artifact");
    intent.request_digest = format!("{:x}", Sha256::digest(&payload));
    store
        .ingest_artifact(ArtifactIngestRequest {
            intent,
            payload,
            evidence_id: "decision-kind-evidence".to_string(),
            object_id: "decision-kind-artifact".to_string(),
            object_kind: "decision".to_string(),
            domain_id: MEMORY.to_string(),
            source_kind: "fixture".to_string(),
            source_id: "decision-kind-artifact".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: sensitivity,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: Some(RepositoryProvenance {
                repository_id: "repo".to_string(),
                revision: "abc123".to_string(),
            }),
        })
        .unwrap()
        .evidence_id
}

/// Domain decision reads enforce artifact sensitivity even when the artifact's `object_kind` is `decision`: a replay
/// that tightens the artifact to secret withholds it from the full read and from the domain read alike.
#[test]
fn the_domain_selection_folds_the_evidence_class_of_a_decision_kind_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 3, 0, 0);
    let evidence_id = ingest_decision_kind_artifact(&store, Sensitivity::Normal);
    store
        .commit(intent("admit-decision-kind-artifact"), |envelope| {
            envelope.record_admission(AdmissionRequest {
                candidate_id: None,
                subject_object_id: Some("decision-kind-artifact".to_string()),
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                event: AdmissionEvent {
                    kind: EventKind::Other,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: Some(evidence_id.clone()),
                    reason: "fixture".to_string(),
                },
            })?;
            Ok(String::new())
        })
        .unwrap();
    let served = |surface: Surface| {
        let tip = store.tip().unwrap();
        let mut full = store
            .visible_as_of_in_scope(surface, tip, None, None)
            .unwrap();
        full.rows
            .retain(|row| row.object.object_kind == "decision" && row.object.domain_id == MEMORY);
        let selected = store
            .visible_domain_decisions_as_of_in_scope(surface, tip, MEMORY, None)
            .unwrap();
        (full.rows, selected.rows)
    };
    let names_artifact = |rows: &[kernel::VisibleRow]| {
        rows.iter()
            .any(|row| row.object.object_id == "decision-kind-artifact")
    };
    let (full, selected) = served(Surface::ExplicitSearch);
    assert!(
        names_artifact(&full) && names_artifact(&selected),
        "the fixture must serve the artifact before the replay"
    );

    ingest_decision_kind_artifact(&store, Sensitivity::Secret);
    for surface in Surface::ALL {
        let (full, selected) = served(*surface);
        assert!(!names_artifact(&full), "{surface:?}");
        assert_eq!(selected, full, "{surface:?}");
    }
}

/// Production-shape timing for the memory read over a store where other domains and approved dependents
/// outnumber memory decisions. Run in release with `--ignored --nocapture`.
#[test]
#[ignore = "builds a large kernel; run in release with --ignored"]
fn canonical_read_at_production_shape() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 1_000, 8_000, 1_000);
    let tip = store.tip().unwrap();
    let median = |mut samples: Vec<u128>| {
        samples.sort_unstable();
        samples[samples.len() / 2]
    };
    let time = |run: &dyn Fn() -> usize| {
        run();
        median(
            (0..15)
                .map(|_| {
                    let started = Instant::now();
                    assert!(run() > 0);
                    started.elapsed().as_micros()
                })
                .collect(),
        )
    };
    let full = time(&|| {
        store
            .visible_as_of_in_scope(Surface::ExplicitSearch, tip, None, None)
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.object.object_kind == "decision" && row.object.domain_id == MEMORY)
            .count()
    });
    let selected = time(&|| {
        store
            .visible_domain_decisions_as_of_in_scope(Surface::ExplicitSearch, tip, MEMORY, None)
            .unwrap()
            .rows
            .len()
    });
    eprintln!("median us: full read then filter {full}, domain selection {selected}");
}

/// Commits `count` admitted decisions of `domain_id` with summaries of `summary_bytes` bytes, after every row already
/// in the store, so they lead the newest-first serving order.
fn commit_decisions(
    store: &KernelStore,
    prefix: &str,
    domain_id: &str,
    count: usize,
    summary_bytes: usize,
) {
    for chunk in (0..count).collect::<Vec<_>>().chunks(256) {
        store
            .commit(intent(&format!("{prefix}-{}", chunk[0])), |envelope| {
                for n in chunk {
                    let object = format!("{prefix}-{n}");
                    let mut spec = decision(&object, domain_id, "preference");
                    spec.payload.summary = "s".repeat(summary_bytes);
                    envelope.insert_decision(spec)?;
                    envelope.record_admission(explicit(&object))?;
                }
                Ok(String::new())
            })
            .unwrap();
    }
}

/// The rank of `object` in the newest-first serving order (`created_commit_seq` descending, then `object_id`).
fn serving_rank(visible: &VisibleAsOf, object: &str) -> usize {
    let mut rows: Vec<(i64, &str)> = visible
        .rows
        .iter()
        .map(|row| (row.object.created_commit_seq, row.object.object_id.as_str()))
        .collect();
    rows.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
    rows.iter().position(|(_, id)| *id == object).unwrap() + 1
}

/// Row pressure: more than 8,192 newer visible decisions of another domain put the memory target past rank 8,192 of
/// the unfiltered serving order, under 8 MiB of payload, while the domain selection returns it among its own rows.
#[test]
fn the_domain_selection_reaches_a_target_past_the_row_cap_of_the_unfiltered_order() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 3, 0, 0);
    commit_decisions(&store, "pressure", "noise", 8_300, 16);
    let tip = store.tip().unwrap();
    let all = store
        .visible_as_of_in_scope(Surface::ExplicitSearch, tip, None, None)
        .unwrap();
    assert!(serving_rank(&all, "memory-0") > 8_192);
    let payload_bytes: i64 = Connection::open(directory.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT sum(length(decision_payload)) FROM decisions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(payload_bytes < 8 * 1024 * 1024, "{payload_bytes}");
    let selected = store
        .visible_domain_decisions_as_of_in_scope(Surface::ExplicitSearch, tip, MEMORY, None)
        .unwrap();
    assert_eq!(selected.rows.len(), 3);
    assert_eq!(serving_rank(&selected, "memory-0"), 1);
}

/// Byte pressure: more than 8 MiB of stored payload in newer decisions of another domain precede the memory target
/// in serving order, under 8,192 rows, while the domain selection reads none of those rows.
#[test]
fn the_domain_selection_reads_no_row_of_another_domains_payload_pressure() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 3, 0, 0);
    commit_decisions(&store, "bytes", "noise", 140, 64 * 1024);
    let tip = store.tip().unwrap();
    let all = store
        .visible_as_of_in_scope(Surface::ExplicitSearch, tip, None, None)
        .unwrap();
    assert!(all.rows.len() < 8_192);
    let noise_bytes: i64 = Connection::open(directory.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT sum(length(d.decision_payload)) FROM decisions d
             JOIN object_registry o ON o.object_id=d.object_id WHERE o.domain_id='noise'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(noise_bytes > 8 * 1024 * 1024, "{noise_bytes}");
    assert!(serving_rank(&all, "memory-0") > 140);
    let selected = store
        .visible_domain_decisions_as_of_in_scope(Surface::ExplicitSearch, tip, MEMORY, None)
        .unwrap();
    assert_eq!(selected.rows.len(), 3);
    assert!(
        selected
            .rows
            .iter()
            .all(|row| row.object.domain_id == MEMORY)
    );
}
