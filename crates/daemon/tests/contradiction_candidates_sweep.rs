use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;

use daemon::contradiction_candidates::Candidates;
use daemon::contradiction_candidates::eligibility::eligible;
use daemon::contradiction_candidates::scorer::matcher_digest;
use daemon::contradiction_candidates::sweep::{
    CANDIDATE_SWEEP_PAGE, CandidateSweep, MAX_CANDIDATE_AGE,
};
use kernel::applicability::EvalBudget;
use kernel::{
    AdmissionEvent, AdmissionRequest, ArtifactIngestRequest, ClaimFactBounds, ClaimFacts,
    CommitIntent, DecisionPayload, DecisionSpec, Disposition, DomainSpec, EventKind,
    EvidenceLiveness, EvidenceLivenessError, KernelStore, MAX_EVIDENCE_LIVENESS_IDS, ProjectScope,
    ProviderEgress, ScopeSpec, ScopeTermSpec, Sensitivity, SourceClass, TaintClass,
};
use memory_store::MemoryStore;
use memory_store::contradiction_candidates::{
    CandidateCaps, CandidateScore, DescriptorCandidates, StoredCandidate,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PROJECT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_PROJECT: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const NOW_MS: i64 = 1_800_000_000_000;
const CAPS: CandidateCaps = CandidateCaps {
    per_descriptor: 16,
    per_project: 100_000,
};

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "contradiction-candidates-sweep-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn request(
    object_id: &str,
    kind: EventKind,
    classes: (SourceClass, TaintClass),
) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object_id.to_string()),
        source_class: Some(classes.0),
        taint_class: Some(classes.1),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "fixture".to_string(),
        },
    }
}

const TRUSTED: (SourceClass, TaintClass) = (SourceClass::TrustedLocalCode, TaintClass::CurrentCode);
fn decision(name: &str, domain: &str, scope: &str) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{name}"),
        object_id: format!("object-{name}"),
        domain_id: domain.to_string(),
        proposition_id: None,
        scope_id: Some(scope.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("decision {name}"),
            rationale: "fixture".to_string(),
        },
        source_kind: "fixture".to_string(),
        source_id: format!("lineage-{name}"),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

fn scope(project: &str) -> ScopeSpec {
    ScopeSpec {
        scope_id: format!("project:{project}"),
        object_id: format!("project:{project}"),
        domain_id: "memory".to_string(),
        source_kind: "kernel_route".to_string(),
        source_id: format!("project:{project}"),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: vec![ScopeTermSpec {
            dimension: "project".to_string(),
            operator: "exact".to_string(),
            exact_value: Some(project.to_string()),
            ..ScopeTermSpec::default()
        }],
    }
}

fn domain(id: &str) -> DomainSpec {
    DomainSpec {
        domain_id: id.to_string(),
        object_id: format!("domain-{id}"),
        name: id.to_string(),
        source_kind: "fixture".to_string(),
        source_id: id.to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
    }
}

const DECISIONS: [&str; 10] = [
    "active",
    "stale",
    "disputed",
    "superseded",
    "contradicted",
    "rejected",
    "quarantined",
    "invalidated",
    "other-project",
    "other-domain",
];

const ELIGIBLE: [&str; 3] = ["active", "stale", "disputed"];

struct Fixture {
    dir: tempfile::TempDir,
    kernel: KernelStore,
    store: MemoryStore,
    live_evidence: String,
    retired_evidence: String,
}

impl Fixture {
    fn open() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("kernel")).unwrap();
        let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
        let store = MemoryStore::open_for_test(dir.path(), "sweep");
        kernel
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(domain("memory"))?;
                envelope.insert_domain(domain("notes"))?;
                envelope.insert_scope(scope(PROJECT))?;
                envelope.insert_scope(scope(OTHER_PROJECT))?;
                let project = format!("project:{PROJECT}");
                for name in DECISIONS {
                    let spec = match name {
                        "other-project" => {
                            decision(name, "memory", &format!("project:{OTHER_PROJECT}"))
                        }
                        "other-domain" => decision(name, "notes", &project),
                        _ => decision(name, "memory", &project),
                    };
                    envelope.insert_decision(spec)?;
                    envelope.record_admission(request(
                        &format!("object-{name}"),
                        EventKind::Other,
                        TRUSTED,
                    ))?;
                }
                Ok(String::new())
            })
            .unwrap();
        let events = [
            ("stale", EventKind::MarkStale, TRUSTED),
            ("disputed", EventKind::MarkDisputed, TRUSTED),
            ("contradicted", EventKind::Contradict, TRUSTED),
            ("rejected", EventKind::ExplicitReject, TRUSTED),
            ("quarantined", EventKind::Quarantine, TRUSTED),
        ];
        for (name, kind, classes) in events {
            kernel
                .commit(intent(&format!("event-{name}")), |envelope| {
                    envelope.record_admission(request(&format!("object-{name}"), kind, classes))?;
                    Ok(String::new())
                })
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
        kernel
            .commit(intent("supersede"), |envelope| {
                let mut successor = decision("successor", "memory", &format!("project:{PROJECT}"));
                successor.source_id = "lineage-superseded".to_string();
                successor.source_revision = 2;
                envelope.correct_decision("object-superseded", successor)?;
                envelope.retire_decision("object-invalidated")?;
                Ok(String::new())
            })
            .unwrap();
        let live_evidence = ingest(&kernel, "live");
        let retired_evidence = ingest(&kernel, "retired");
        kernel
            .commit(intent("retire-evidence"), |envelope| {
                envelope.retire_evidence("evidence-object-retired")?;
                Ok(String::new())
            })
            .unwrap();
        Self {
            dir,
            kernel,
            store,
            live_evidence,
            retired_evidence,
        }
    }

    fn incarnation(&self) -> String {
        self.kernel
            .capture_identified_commit_read_target_within_budget(&EvalBudget::unbounded())
            .unwrap()
            .database_incarnation_id
    }

    fn facts(&self, name: &str) -> ClaimFacts {
        let target = self
            .kernel
            .capture_commit_read_target_within_budget(&EvalBudget::unbounded())
            .unwrap();
        let bounds = ClaimFactBounds {
            max_claims: NonZeroUsize::new(1).unwrap(),
            max_causal_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        };
        self.kernel
            .claim_facts_at(&[format!("object-{name}")], target, bounds)
            .unwrap()
            .claims
            .remove(0)
    }

    fn write(&self, project: &str, evidence: &str, decisions: &[&str], tweak: impl Fn(&mut Row)) {
        let mut row = Row {
            incarnation: self.incarnation(),
            digest: matcher_digest().to_string(),
            created_at_ms: NOW_MS,
        };
        tweak(&mut row);
        let ids: Vec<(String, String)> = decisions
            .iter()
            .map(|name| (format!("object-{name}"), format!("decision-{name}")))
            .collect();
        let scores: Vec<CandidateScore<'_>> = ids
            .iter()
            .map(|(object, decision)| CandidateScore {
                decision_object_id: object,
                decision_id: decision,
                matched_terms: 3,
                decision_terms: 4,
            })
            .collect();
        self.store
            .replace_candidates_for_descriptor(
                &DescriptorCandidates {
                    project_digest: project,
                    evidence_id: evidence,
                    publish_tip: 1,
                    kernel_incarnation_id: &row.incarnation,
                    matcher_digest: &row.digest,
                    created_at_ms: row.created_at_ms,
                    rows: &scores,
                },
                CAPS,
            )
            .unwrap();
    }

    fn rows(&self) -> Vec<StoredCandidate> {
        self.store
            .candidate_sweep_page(0, 4096)
            .unwrap()
            .into_iter()
            .map(|(_, row)| row)
            .collect()
    }

    fn tip(&self) -> i64 {
        self.kernel
            .capture_commit_read_target_within_budget(&EvalBudget::unbounded())
            .unwrap()
            .through_commit
    }
}

struct Row {
    incarnation: String,
    digest: String,
    created_at_ms: i64,
}

fn ingest(kernel: &KernelStore, key: &str) -> String {
    let payload = format!("descriptor bytes {key}").into_bytes();
    let mut intent = intent(&format!("evidence-{key}"));
    intent.request_digest = format!("{:x}", Sha256::digest(&payload));
    kernel
        .ingest_exact_artifact(ArtifactIngestRequest {
            intent,
            payload,
            evidence_id: format!("evidence-{key}"),
            object_id: format!("evidence-object-{key}"),
            object_kind: "evidence".to_string(),
            domain_id: "memory".to_string(),
            source_kind: "tool_output".to_string(),
            source_id: format!("native/{key}"),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: Sensitivity::Normal,
            provider_egress: ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap()
        .evidence_id
}

fn report(counts: &Candidates) -> serde_json::Map<String, Value> {
    counts.report().as_object().unwrap().clone()
}

fn kept(fixture: &Fixture) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = fixture
        .rows()
        .into_iter()
        .map(|row| (row.project_digest, row.evidence_id, row.decision_object_id))
        .collect();
    rows.sort();
    rows
}

#[test]
fn the_eligibility_rule_admits_exactly_active_stale_and_disputed_decisions_of_the_project() {
    let fixture = Fixture::open();
    let project = ProjectScope::new(PROJECT).unwrap();
    let expected = [
        ("active", Disposition::Active),
        ("stale", Disposition::Stale),
        ("disputed", Disposition::Disputed),
        ("contradicted", Disposition::Contradicted),
        ("rejected", Disposition::Rejected),
        ("quarantined", Disposition::Quarantined),
    ];
    for (name, disposition) in expected {
        let facts = fixture.facts(name);
        assert_eq!(
            facts.own_admission.as_ref().map(|own| own.disposition),
            Some(disposition),
            "{name}"
        );
    }
    let superseded = fixture.facts("superseded");
    assert_eq!(
        superseded.object.superseded_by.as_deref(),
        Some("object-successor")
    );
    for name in DECISIONS {
        let facts = fixture.facts(name);
        let terms = facts
            .decision
            .scope_id
            .as_deref()
            .map(|scope| fixture.kernel.scope_terms(scope).unwrap());
        let names_project = project.names_project(terms.as_deref());
        assert_eq!(
            eligible(&facts, names_project),
            ELIGIBLE.contains(&name),
            "{name}"
        );
    }
}

#[test]
fn one_sweep_drops_every_dead_row_keeps_the_eligible_ones_and_a_repeat_drops_none() {
    let fixture = Fixture::open();
    let live = fixture.live_evidence.clone();
    fixture.write(PROJECT, &live, &DECISIONS, |_| {});
    let retired = fixture.retired_evidence.clone();
    fixture.write(PROJECT, &retired, &["active"], |_| {});
    fixture.write(OTHER_PROJECT, &live, &["other-project", "active"], |_| {});
    fixture.write(PROJECT, "evidence-foreign", &["active"], |row| {
        row.incarnation = "0".repeat(32);
    });
    fixture.write(PROJECT, "evidence-stale-digest", &["active"], |row| {
        row.digest = "f".repeat(64);
    });
    let age_ms = i64::try_from(MAX_CANDIDATE_AGE.as_millis()).unwrap();
    fixture.write(
        OTHER_PROJECT,
        "evidence-over-age",
        &["other-project"],
        |row| {
            row.created_at_ms = NOW_MS - age_ms - 1;
        },
    );
    let seeded = fixture.rows().len();
    let tip = fixture.tip();
    let jobs = fixture.store.memory_reviewer_status_facts().unwrap();
    let counts = Candidates::default();
    let mut sweep = CandidateSweep::default();
    sweep.run(&fixture.store, Some(&fixture.kernel), true, &counts, NOW_MS);

    let mut expected: Vec<(String, String, String)> = ELIGIBLE
        .iter()
        .map(|name| (PROJECT.to_string(), live.clone(), format!("object-{name}")))
        .collect();
    expected.push((
        OTHER_PROJECT.to_string(),
        live.clone(),
        "object-other-project".to_string(),
    ));
    expected.sort();
    assert_eq!(kept(&fixture), expected);
    let block = report(&counts);
    assert_eq!(block["sweep_examined"], seeded);
    assert_eq!(block["sweep_dropped"], seeded - expected.len());
    assert_eq!(block["sweep_skipped_kernel_unavailable"], 0);
    let (_, bytes) = fixture.store.candidate_rows_and_bytes(None).unwrap();
    assert_eq!(block["retained_bytes"], bytes);

    sweep.run(&fixture.store, Some(&fixture.kernel), true, &counts, NOW_MS);
    assert_eq!(
        kept(&fixture),
        expected,
        "a repeat at the same tip drops none"
    );
    assert_eq!(report(&counts)["sweep_dropped"], seeded - expected.len());
    assert_eq!(fixture.tip(), tip, "the sweep commits nothing");
    assert_eq!(
        fixture.store.memory_reviewer_status_facts().unwrap(),
        jobs,
        "the sweep files no reviewer job"
    );
}

#[test]
fn a_row_at_the_age_bound_stays_and_one_past_it_drops() {
    let fixture = Fixture::open();
    let live = fixture.live_evidence.clone();
    let age_ms = i64::try_from(MAX_CANDIDATE_AGE.as_millis()).unwrap();
    fixture.write(PROJECT, &live, &["active"], |row| {
        row.created_at_ms = NOW_MS - age_ms;
    });
    let counts = Candidates::default();
    let mut sweep = CandidateSweep::default();
    sweep.run(&fixture.store, Some(&fixture.kernel), true, &counts, NOW_MS);
    assert_eq!(fixture.rows().len(), 1);
    sweep.run(
        &fixture.store,
        Some(&fixture.kernel),
        true,
        &counts,
        NOW_MS + 1,
    );
    assert_eq!(fixture.rows().len(), 0);
}

#[test]
fn a_closed_kernel_drops_nothing_and_counts_the_skip() {
    let fixture = Fixture::open();
    fixture.write(PROJECT, "evidence-foreign", &["active"], |row| {
        row.incarnation = "0".repeat(32);
    });
    let counts = Candidates::default();
    CandidateSweep::default().run(&fixture.store, None, true, &counts, NOW_MS);
    assert_eq!(fixture.rows().len(), 1);
    let block = report(&counts);
    assert_eq!(block["sweep_skipped_kernel_unavailable"], 1);
    assert_eq!(block["sweep_examined"], 0);
    assert_eq!(block["sweep_dropped"], 0);
}

#[test]
fn the_watermark_covers_three_pages_in_three_runs_and_the_fourth_starts_over() {
    let fixture = Fixture::open();
    let live = fixture.live_evidence.clone();
    fixture.write(PROJECT, &live, &["active"], |_| {});
    for index in 1..3 * CANDIDATE_SWEEP_PAGE {
        fixture.write(PROJECT, &format!("evidence-{index}"), &["active"], |_| {});
    }
    let counts = Candidates::default();
    let mut sweep = CandidateSweep::default();
    for run in 1..=3 {
        sweep.run(&fixture.store, Some(&fixture.kernel), true, &counts, NOW_MS);
        assert_eq!(
            report(&counts)["sweep_examined"],
            run * CANDIDATE_SWEEP_PAGE
        );
    }
    assert_eq!(
        fixture.rows().len(),
        1,
        "rows citing unknown evidence drop; the live descriptor's row stays"
    );
    sweep.run(&fixture.store, Some(&fixture.kernel), true, &counts, NOW_MS);
    assert_eq!(
        report(&counts)["sweep_examined"],
        3 * CANDIDATE_SWEEP_PAGE + 1,
        "the fourth run starts over at the first row"
    );
}

#[test]
fn a_false_flag_runs_no_table_statement_and_moves_no_counter() {
    let fixture = Fixture::open();
    fixture.write(PROJECT, "evidence-foreign", &["active"], |row| {
        row.incarnation = "0".repeat(32);
    });
    let path = fixture.dir.path().join("memory.sqlite");
    let rename = |from: &str, to: &str| {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}"))
            .unwrap();
    };
    rename(
        "contradiction_candidates",
        "contradiction_candidates_hidden",
    );
    let counts = Candidates::default();
    let mut sweep = CandidateSweep::default();
    for _ in 0..3 {
        sweep.run(
            &fixture.store,
            Some(&fixture.kernel),
            false,
            &counts,
            NOW_MS,
        );
        sweep.run(&fixture.store, None, false, &counts, NOW_MS);
    }
    assert!(
        report(&counts).values().all(|value| value == 0),
        "{:?}",
        report(&counts)
    );
    sweep.run(&fixture.store, Some(&fixture.kernel), true, &counts, NOW_MS);
    assert_eq!(
        report(&counts)["store_unavailable"],
        1,
        "the hidden table fails any statement the sweep runs"
    );
    rename(
        "contradiction_candidates_hidden",
        "contradiction_candidates",
    );
    assert_eq!(fixture.rows().len(), 1);
}

#[test]
fn the_counter_block_holds_exactly_the_closed_vocabulary_as_integers() {
    let names = [
        "offered",
        "admitted",
        "refused_pass_busy",
        "refused_shutting_down",
        "store_unavailable",
        "kernel_unavailable",
        "descriptors_analyzed",
        "descriptors_refused_oversize",
        "descriptors_unscored",
        "candidates_truncated",
        "summaries_loaded",
        "summaries_cut_bytes",
        "summaries_unscorable",
        "budget_exhausted",
        "rows_written",
        "dropped_at_cap",
        "sweep_examined",
        "sweep_dropped",
        "sweep_skipped_kernel_unavailable",
        "read_filtered_stale",
        "completed",
        "deadline",
        "worker_panicked",
        "active",
        "retained_bytes",
        "pass_ms_last",
        "pass_ms_max",
    ];
    let block = report(&Candidates::default());
    let mut keys: Vec<&str> = block.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = names.to_vec();
    expected.sort_unstable();
    assert_eq!(keys, expected);
    assert!(block.values().all(Value::is_u64));
}

#[test]
fn the_evidence_read_answers_liveness_at_its_snapshot_in_request_order() {
    let fixture = Fixture::open();
    let before = fixture
        .kernel
        .capture_commit_read_target_within_budget(&EvalBudget::unbounded())
        .unwrap();
    let late = ingest(&fixture.kernel, "late");
    let after = fixture
        .kernel
        .capture_commit_read_target_within_budget(&EvalBudget::unbounded())
        .unwrap();
    let ids = [
        fixture.retired_evidence.as_str(),
        "evidence-never",
        late.as_str(),
        fixture.live_evidence.as_str(),
    ];
    let budget = EvalBudget::unbounded();
    assert_eq!(
        fixture
            .kernel
            .evidence_liveness_at_within_budget(&ids, before, &budget)
            .unwrap(),
        [
            EvidenceLiveness::Retired,
            EvidenceLiveness::Unknown,
            EvidenceLiveness::Unknown,
            EvidenceLiveness::Live
        ]
    );
    assert_eq!(
        fixture
            .kernel
            .evidence_liveness_at_within_budget(&ids, after, &budget)
            .unwrap()[2],
        EvidenceLiveness::Live
    );
    let too_many = vec!["evidence-never"; MAX_EVIDENCE_LIVENESS_IDS + 1];
    assert_eq!(
        fixture
            .kernel
            .evidence_liveness_at_within_budget(&too_many, after, &budget),
        Err(EvidenceLivenessError::TooManyIds)
    );
    let expired = EvalBudget::new(Some(std::time::Instant::now()), Arc::default());
    assert_eq!(
        fixture
            .kernel
            .evidence_liveness_at_within_budget(&ids, after, &expired),
        Err(EvidenceLivenessError::Kernel(kernel::KernelError::Deadline))
    );
}

#[test]
fn the_candidate_modules_name_no_kernel_write_path() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/contradiction_candidates");
    let writes = [
        ".commit(",
        "commit_within_budget",
        "Envelope",
        "CommitIntent",
        "record_admission",
        "record_dispute_admission",
        "ingest_",
        "CausalInputs",
        "enqueue",
    ];
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for write in writes {
            assert!(!text.contains(write), "{} names {write}", path.display());
        }
    }
}

#[test]
fn every_reviewer_job_the_daemon_builds_carries_no_signal() {
    let mut pending = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut built = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split("#[cfg(test)]").next().unwrap();
            for (at, _) in production.match_indices("CausalInputs {") {
                let mut depth = 0;
                let end = production[at..]
                    .char_indices()
                    .find(|(_, ch)| {
                        depth += match ch {
                            '{' => 1,
                            '}' => -1,
                            _ => 0,
                        };
                        *ch == '}' && depth == 0
                    })
                    .unwrap()
                    .0;
                let body = &production[at..=at + end];
                assert!(
                    body.contains("signals: Vec::new()"),
                    "{} builds CausalInputs with a signal: {body}",
                    path.display()
                );
                built += 1;
            }
        }
    }
    assert!(
        built > 0,
        "the scan found the daemon's CausalInputs constructions"
    );
}
