#![cfg(feature = "test-support")]
//! Memory-domain reads over a store whose other domains and approved dependents outnumber its memory decisions.

use std::collections::HashMap;
use std::time::Instant;

use kernel::{
    AdmissionDomainSpec, AdmissionEvent, AdmissionRequest, ArtifactIngestRequest, ChunkRow,
    CommitIntent, DecisionContent, DecisionPayload, DecisionRow, DecisionSpec, DomainSpec,
    EventKind, KernelError, KernelStore, ProviderEgress, RepositoryProvenance, Sensitivity,
    ServingPosition, SourceClass, StagingCandidateSpec, Surface, TaintClass, VisibleAsOf,
    VisibleRow,
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

/// The rows of a full read in serving order: newest commit first, then `object_id` ascending.
fn serving_order(visible: &VisibleAsOf) -> Vec<VisibleRow> {
    let mut rows = visible.rows.clone();
    rows.sort_by(|left, right| {
        right
            .object
            .created_commit_seq
            .cmp(&left.object.created_commit_seq)
            .then_with(|| left.object.object_id.cmp(&right.object.object_id))
    });
    rows
}

fn visible_rows(rows: &[ChunkRow]) -> Vec<VisibleRow> {
    rows.iter().map(|entry| entry.row.clone()).collect()
}

fn position(row: &VisibleRow) -> ServingPosition<'_> {
    ServingPosition {
        created_commit_seq: row.object.created_commit_seq,
        object_id: &row.object.object_id,
    }
}

fn payload_bytes(rows: &[ChunkRow]) -> u64 {
    rows.iter()
        .filter_map(|entry| entry.decision.as_ref())
        .map(|decision| decision.payload_bytes)
        .sum()
}

/// Chunks read back to back serve the full read's rows in serving order at every chunk size. A chunk that leaves
/// rows unread ends with a whole commit, holds `min_rows` rows, and would hold fewer without its last commit.
#[test]
fn chunks_serve_the_full_read_in_serving_order_and_stop_at_a_commit_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 40, 300, 4);
    let tip = store.tip().unwrap();
    for (surface, requested) in [
        (Surface::AutoInject, tip),
        (Surface::ExplicitSearch, tip),
        (Surface::ExplicitSearch, tip - 2),
    ] {
        let all = store
            .visible_as_of_in_scope(surface, requested, None, None)
            .unwrap();
        let expected = serving_order(&all);
        let mut commits: Vec<i64> = expected
            .iter()
            .map(|row| row.object.created_commit_seq)
            .collect();
        commits.dedup();
        assert!(commits.len() > 2, "{surface:?}: {commits:?}");
        // A request for no rows reads as a request for one.
        for min_rows in [0, 1, 2, 41, 257, 10_000] {
            let filled = min_rows.max(1);
            let mut served: Vec<VisibleRow> = Vec::new();
            let mut chunks = 0;
            loop {
                let chunk = store
                    .visible_chunk_as_of_in_scope(
                        surface,
                        requested,
                        served.last().map(position),
                        min_rows,
                        u64::MAX,
                        None,
                    )
                    .unwrap();
                assert_eq!(
                    (chunk.known_as_of, chunk.tip),
                    (all.known_as_of, all.tip),
                    "{surface:?} min_rows {min_rows}"
                );
                chunks += 1;
                let rows = visible_rows(&chunk.rows);
                let last_commit = rows.last().map(|row| row.object.created_commit_seq);
                let before_last_commit = rows
                    .iter()
                    .filter(|row| Some(row.object.created_commit_seq) != last_commit)
                    .count();
                let len = rows.len();
                served.extend(rows);
                if chunk.exhausted {
                    break;
                }
                assert!(len >= filled, "{surface:?} min_rows {min_rows}: {len}");
                assert!(
                    before_last_commit < filled,
                    "{surface:?} min_rows {min_rows}: the chunk read past the commit that filled it"
                );
                if let Some(next) = expected.get(served.len()) {
                    assert!(
                        Some(next.object.created_commit_seq) < last_commit,
                        "{surface:?} min_rows {min_rows}: the chunk split a commit"
                    );
                }
            }
            assert_eq!(served, expected, "{surface:?} min_rows {min_rows}");
            if filled == 1 {
                assert!(
                    chunks >= commits.len(),
                    "{surface:?}: {chunks} chunks over {} commits",
                    commits.len()
                );
            }
        }
    }
}

/// A position inside a commit serves the rest of that commit by `object_id` and then every older commit; a
/// position ahead of every row serves the whole read, and the oldest row's position serves nothing.
#[test]
fn a_chunk_after_a_position_serves_exactly_the_rows_that_follow_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 40, 300, 4);
    let tip = store.tip().unwrap();
    for surface in [Surface::AutoInject, Surface::ExplicitSearch] {
        let all = store
            .visible_as_of_in_scope(surface, tip, None, None)
            .unwrap();
        let expected = serving_order(&all);
        let after = |position: ServingPosition<'_>| {
            let chunk = store
                .visible_chunk_as_of_in_scope(
                    surface,
                    tip,
                    Some(position),
                    usize::MAX,
                    u64::MAX,
                    None,
                )
                .unwrap();
            assert!(chunk.exhausted);
            visible_rows(&chunk.rows)
        };
        let inside_a_commit = expected
            .windows(2)
            .position(|pair| pair[0].object.created_commit_seq == pair[1].object.created_commit_seq)
            .unwrap_or(0);
        if surface == Surface::ExplicitSearch {
            assert_eq!(
                expected[inside_a_commit].object.created_commit_seq,
                expected[inside_a_commit + 1].object.created_commit_seq,
                "the explicit-search rows hold a commit with two visible rows"
            );
        }
        for index in [inside_a_commit, expected.len() / 2, expected.len() - 1] {
            assert_eq!(
                after(position(&expected[index])),
                expected[index + 1..],
                "{surface:?} after {index}"
            );
        }
        let ahead = ServingPosition {
            created_commit_seq: tip + 1,
            object_id: "",
        };
        assert_eq!(after(ahead), expected, "{surface:?}");
        // A position names a row by commit and id, so an id no row carries still splits its commit.
        let first = &expected[inside_a_commit];
        let between = format!("{}\u{0}", first.object.object_id);
        let unnamed = ServingPosition {
            created_commit_seq: first.object.created_commit_seq,
            object_id: &between,
        };
        assert_eq!(
            after(unnamed),
            expected[inside_a_commit + 1..],
            "{surface:?}"
        );
    }
}

/// A chunk carries the decision of each decision object: its payload size as stored and, inside the payload
/// budget, its kind and the stored payload, which is the JSON `serde_json` encodes for the payload a read by id
/// returns. An object that is no decision carries none. A decision whose stored payload does not decode carries
/// those bytes too, so a caller that serves decoded payloads finds it and the read by id reports it.
#[test]
fn a_chunk_carries_each_decision_object_with_its_decision() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 40, 300, 4);
    // One payload holds every kind of character the stored encoding treats differently.
    store
        .commit(intent("escaped"), |envelope| {
            let mut spec = decision("memory-escaped", MEMORY, "preference");
            spec.payload.summary = "quote \" slash \\ tab \t bell \u{7} é \u{1F600}".to_string();
            spec.payload.rationale = "line\nbreak \u{7f}".to_string();
            envelope.insert_decision(spec)?;
            envelope.record_admission(explicit("memory-escaped"))?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip().unwrap();
    let chunk = store
        .visible_chunk_as_of_in_scope(
            Surface::ExplicitSearch,
            tip,
            None,
            usize::MAX,
            u64::MAX,
            None,
        )
        .unwrap();
    assert!(chunk.exhausted);
    // The fixture's approval decision stores `{}`, which is no decision payload.
    let undecodable = "approval".to_string();
    assert_eq!(
        store.decisions_for_objects_as_of(std::slice::from_ref(&undecodable), tip),
        Err(KernelError::CorruptCanonicalRow)
    );
    let ids: Vec<String> = chunk
        .rows
        .iter()
        .filter(|entry| entry.row.object.object_kind == "decision")
        .map(|entry| entry.row.object.object_id.clone())
        .filter(|id| *id != undecodable)
        .collect();
    assert_eq!(ids.len(), 341);
    assert!(
        ids.len() + 1 < chunk.rows.len(),
        "the surface serves an object that is no decision"
    );
    let stored: HashMap<String, DecisionRow> = store
        .decisions_for_objects_as_of(&ids, tip)
        .unwrap()
        .into_iter()
        .map(|decision| (decision.object_id.clone(), decision))
        .collect();
    let sizes: HashMap<String, u64> = store
        .decision_payload_sizes_as_of(&ids, tip)
        .unwrap()
        .into_iter()
        .collect();
    for entry in &chunk.rows {
        let id = &entry.row.object.object_id;
        if entry.row.object.object_kind != "decision" {
            assert_eq!(entry.decision, None, "{id}");
            continue;
        }
        let decision = entry.decision.as_ref().expect(id);
        let stored_payload = decision
            .content
            .as_ref()
            .map(|content| &content.stored_payload);
        if *id == undecodable {
            assert_eq!(
                (decision.payload_bytes, stored_payload),
                (2, Some(&b"{}".to_vec()))
            );
            continue;
        }
        assert_eq!(decision.payload_bytes, sizes[id], "{id}");
        let content = DecisionContent {
            decision_kind: stored[id].decision_kind.clone(),
            stored_payload: serde_json::to_vec(&stored[id].payload).unwrap(),
        };
        assert_eq!(decision.content.as_ref(), Some(&content), "{id}");
    }
}

/// A decision object's chunk row carries its decision while the decision row is live at the snapshot: created
/// at or before it and invalidated after it. At any other snapshot the row carries none, which a caller that
/// serves the object reads as a missing canonical row.
#[test]
fn a_chunk_carries_a_decision_only_while_its_row_is_live_at_the_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 2, 0, 0);
    let seeded = store.tip().unwrap();
    for name in ["later-one", "later-two"] {
        store
            .commit(intent(name), |envelope| {
                envelope.insert_domain(domain(name))?;
                Ok(String::new())
            })
            .unwrap();
    }
    drop(store);
    // The registry rows keep their commits; only the decision rows move, as a damaged store could hold them.
    Connection::open(directory.path().join("kernel.sqlite"))
        .unwrap()
        .execute_batch(&format!(
            "UPDATE decisions SET created_commit_seq={} WHERE object_id='memory-0';
             UPDATE decisions SET invalidated_commit_seq={} WHERE object_id='memory-1';",
            seeded + 1,
            seeded + 2
        ))
        .unwrap();
    let store = KernelStore::open(directory.path()).unwrap();
    let carried = |requested: i64, object_id: &str| {
        let chunk = store
            .visible_chunk_as_of_in_scope(
                Surface::ExplicitSearch,
                requested,
                None,
                usize::MAX,
                u64::MAX,
                None,
            )
            .unwrap();
        let row = chunk
            .rows
            .iter()
            .find(|entry| entry.row.object.object_id == object_id)
            .unwrap_or_else(|| panic!("{object_id} is visible at {requested}"));
        row.decision
            .as_ref()
            .map(|decision| decision.content.is_some())
    };
    assert_eq!(
        carried(seeded, "memory-0"),
        None,
        "created after the snapshot"
    );
    assert_eq!(
        carried(seeded + 1, "memory-0"),
        Some(true),
        "created at the snapshot"
    );
    assert_eq!(
        carried(seeded + 1, "memory-1"),
        Some(true),
        "invalidated after the snapshot"
    );
    assert_eq!(
        carried(seeded + 2, "memory-1"),
        None,
        "invalidated at the snapshot"
    );
}

/// A payload budget ends the chunk at the first commit boundary where the rows' payload bytes pass it. The
/// payloads the chunk holds stay within the budget, and every decision keeps its payload size.
#[test]
fn a_payload_budget_ends_the_chunk_at_a_commit_boundary_and_bounds_the_payloads_it_holds() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 40, 300, 4);
    let tip = store.tip().unwrap();
    let read = |after: Option<ServingPosition<'_>>, budget: u64| {
        store
            .visible_chunk_as_of_in_scope(
                Surface::ExplicitSearch,
                tip,
                after,
                usize::MAX,
                budget,
                None,
            )
            .unwrap()
    };
    let all = read(None, u64::MAX);
    let total = payload_bytes(&all.rows);
    let commit_of = |entry: &ChunkRow| entry.row.object.created_commit_seq;
    for budget in [0, 1, total / 7, total / 2] {
        let chunk = read(None, budget);
        assert!(!chunk.exhausted, "budget {budget}");
        assert_eq!(
            visible_rows(&chunk.rows),
            visible_rows(&all.rows[..chunk.rows.len()]),
            "budget {budget}"
        );
        let last_commit = commit_of(chunk.rows.last().unwrap());
        assert!(payload_bytes(&chunk.rows) > budget, "budget {budget}");
        let before_last_commit: Vec<ChunkRow> = chunk
            .rows
            .iter()
            .filter(|entry| commit_of(entry) != last_commit)
            .cloned()
            .collect();
        assert!(
            payload_bytes(&before_last_commit) <= budget,
            "budget {budget}: the chunk read past the commit that passed the budget"
        );
        assert!(
            commit_of(&all.rows[chunk.rows.len()]) < last_commit,
            "budget {budget}: the chunk split a commit"
        );
        let held: u64 = chunk
            .rows
            .iter()
            .filter_map(|entry| entry.decision.as_ref())
            .filter(|decision| decision.content.is_some())
            .map(|decision| decision.payload_bytes)
            .sum();
        assert!(held <= budget, "budget {budget}: held {held}");
        for (entry, full) in chunk.rows.iter().zip(&all.rows) {
            let (decision, stored) = (entry.decision.as_ref(), full.decision.as_ref());
            assert_eq!(
                decision.map(|decision| decision.payload_bytes),
                stored.map(|decision| decision.payload_bytes)
            );
            if let Some(content) = decision.and_then(|decision| decision.content.as_ref()) {
                assert_eq!(
                    Some(content),
                    stored.and_then(|decision| decision.content.as_ref())
                );
            }
        }
        let rest = read(
            chunk.rows.last().map(|entry| position(&entry.row)),
            u64::MAX,
        );
        assert!(rest.exhausted);
        assert_eq!(
            visible_rows(&rest.rows),
            visible_rows(&all.rows[chunk.rows.len()..]),
            "budget {budget}"
        );
    }
    // A budget only the oldest commit passes leaves no commit boundary to stop at.
    for budget in [total - 1, total] {
        let chunk = read(None, budget);
        assert!(chunk.exhausted, "budget {budget}");
        assert_eq!(
            visible_rows(&chunk.rows),
            visible_rows(&all.rows),
            "budget {budget}"
        );
    }
    assert_eq!(read(None, total).rows, all.rows);
}

/// A chunk after a position inside a commit spends its payload budget on the rows it returns: with a budget of
/// exactly their payload bytes it holds every one of their payloads, as the unbounded read does.
#[test]
fn a_chunk_after_a_position_spends_its_payload_budget_on_the_rows_it_returns() {
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 40, 300, 4);
    let tip = store.tip().unwrap();
    let read = |after: Option<ServingPosition<'_>>, budget: u64| {
        store
            .visible_chunk_as_of_in_scope(
                Surface::ExplicitSearch,
                tip,
                after,
                usize::MAX,
                budget,
                None,
            )
            .unwrap()
    };
    let all = read(None, u64::MAX).rows;
    let inside_a_commit = all
        .windows(2)
        .position(|pair| {
            pair[0].row.object.created_commit_seq == pair[1].row.object.created_commit_seq
                && pair[0].decision.is_some()
        })
        .expect("a commit holds a decision ahead of another row");
    let rest = &all[inside_a_commit + 1..];
    let budget = payload_bytes(rest);
    assert!(budget > 0);
    let chunk = read(Some(position(&all[inside_a_commit].row)), budget);
    assert!(chunk.exhausted);
    assert_eq!(chunk.rows, rest);
}

/// The page statement returns a payload with its row up to 64 KiB. A chunk carries a larger decision by its
/// size alone under any budget, and holds the decisions at or under the limit against the budget.
#[test]
fn a_chunk_holds_a_payload_up_to_the_inline_size() {
    const INLINE_BYTES: u64 = 64 * 1024;
    let directory = tempfile::tempdir().unwrap();
    let store = store_with(directory.path(), 0, 0, 0);
    // The larger payload has the greater object id, so the chunk reads it first.
    let sized = [("sized-a", INLINE_BYTES), ("sized-b", INLINE_BYTES + 1)];
    store
        .commit(intent("sized"), |envelope| {
            for (object, bytes) in sized {
                let mut spec = decision(object, MEMORY, "preference");
                spec.payload.rationale = "r".to_string();
                let framing = serde_json::to_vec(&DecisionPayload {
                    summary: String::new(),
                    rationale: spec.payload.rationale.clone(),
                })
                .unwrap()
                .len();
                spec.payload.summary = "s".repeat(bytes as usize - framing);
                envelope.insert_decision(spec)?;
                envelope.record_admission(explicit(object))?;
            }
            Ok(String::new())
        })
        .unwrap();
    let tip = store.tip().unwrap();
    for budget in [u64::MAX, INLINE_BYTES] {
        let chunk = store
            .visible_chunk_as_of_in_scope(
                Surface::ExplicitSearch,
                tip,
                None,
                usize::MAX,
                budget,
                None,
            )
            .unwrap();
        let held: Vec<(u64, bool)> = sized
            .iter()
            .map(|(object, _)| {
                let entry = chunk
                    .rows
                    .iter()
                    .find(|entry| entry.row.object.object_id == *object)
                    .unwrap_or_else(|| panic!("{object} is visible"));
                let decision = entry.decision.as_ref().expect(object);
                (decision.payload_bytes, decision.content.is_some())
            })
            .collect();
        assert_eq!(
            held,
            [(INLINE_BYTES, true), (INLINE_BYTES + 1, false)],
            "budget {budget}"
        );
    }
}
