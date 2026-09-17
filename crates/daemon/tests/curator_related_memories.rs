//! Real-store proofs for bounded related-memory discovery: multi-page keyset traversal at one snapshot, the decisive later-page contradiction, concurrent eligibility changes, invalid and foreign cursors, candidate, probe, and batch bounds, multibyte excerpt spans, and the absence of canonical writes.

use daemon::curator::broker::{
    EvidenceBroker, MAX_OPERATIONS_PER_BATCH, QuestionTemplate, RefusalCode, RunBinding,
};
use daemon::curator::related_memories::{
    MAX_PAGE_PROBE_BYTES, MAX_PROBE_ARTIFACT_BYTES, MAX_RELATED_CANDIDATES_PER_PAGE,
    MAX_RELATED_PAGE_HITS, RelatedHit, RelatedMemoryDiscovery,
};
use daemon::curator::{Completeness, MAX_EXCERPT_BYTES};
use kernel::applicability::EvalBudget;
use kernel::source_identity::Occurrence;
use kernel::{
    ArtifactIngestRequest, CURATOR_CAPTURE_RETENTION_CLASS, CommitIntent, CuratorHoldBinding,
    DecisionPayload, DecisionSpec, Dimension, DomainSpec, KernelStore, ProjectScope,
    ProviderEgress, ScopeSpec, ScopeTermSpec, Sensitivity, SourceDescriptorPolicy,
    SourceDescriptorRequest,
};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_PROJECT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SCOPE: &str = "project:a";
const HOUR_MS: i64 = 60 * 60 * 1_000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "curator-broker-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn incarnation(root: &std::path::Path) -> String {
    rusqlite::Connection::open_with_flags(
        root.join("kernel.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT database_incarnation_id FROM kernel_format_marker",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

struct Fixture {
    directory: tempfile::TempDir,
    store: KernelStore,
    now: i64,
}

impl Fixture {
    fn open() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.insert_scope(ScopeSpec {
                    scope_id: SCOPE.to_string(),
                    object_id: SCOPE.to_string(),
                    source_id: SCOPE.to_string(),
                    domain_id: DOMAIN.to_string(),
                    source_kind: "kernel_route".to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: vec![ScopeTermSpec {
                        dimension: Dimension::Project.as_str().to_string(),
                        operator: "exact".to_string(),
                        exact_value: Some(PROJECT.to_string()),
                        ..ScopeTermSpec::default()
                    }],
                })?;
                Ok(String::new())
            })
            .unwrap();
        Self {
            directory,
            store,
            now: now_ms(),
        }
    }

    fn hold_binding(&self, project: &str) -> CuratorHoldBinding {
        CuratorHoldBinding {
            project_digest: project.to_string(),
            kernel_incarnation: incarnation(self.directory.path()),
            memstore_incarnation: "m".repeat(32),
            subject: "job-1".to_string(),
            generation: 1,
        }
    }

    fn ingest(&self, key: &str, payload: &[u8], capture: bool) -> (String, String) {
        let handle = self
            .store
            .ingest_artifact(ArtifactIngestRequest {
                intent: intent(key),
                payload: payload.to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: if capture {
                    "local_file"
                } else {
                    "conversation"
                }
                .to_string(),
                source_id: format!("src/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: if capture {
                    CURATOR_CAPTURE_RETENTION_CLASS.to_string()
                } else {
                    "canonical".to_string()
                },
                retain_until: capture.then_some(self.now + HOUR_MS),
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    /// A broker whose execution hold already covers `evidence` for this project, disclosing to a local destination.
    fn broker(&self, project: &str, evidence: &[String]) -> EvidenceBroker {
        let binding = self.hold_binding(project);
        let hold = self
            .store
            .acquire_execution_hold(&binding, evidence, self.now + 2 * HOUR_MS)
            .unwrap();
        EvidenceBroker::new(
            RunBinding {
                project: ProjectScope::new(project).unwrap(),
                hold: binding,
                hold_id: hold.hold_id,
                destination: kernel::ArtifactDestination::Local,
            },
            QuestionTemplate::ExtractedFacts,
        )
    }

    fn decision(&self, object: &str) {
        self.decision_at_revision(object, 1);
    }

    fn decision_at_revision(&self, object: &str, source_revision: i64) {
        self.store
            .commit(intent(&format!("decision-{object}")), |envelope| {
                envelope.insert_decision(decision_spec(object, source_revision))?;
                envelope.record_admission(admission(object))?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Supersedes `object` with a new admitted decision at the next revision of its source, which revokes every form derived from the predecessor.
    fn supersede(&self, object: &str, replacement: &str) {
        self.store
            .commit(intent(&format!("supersede-{object}")), |envelope| {
                let mut spec = decision_spec(replacement, 2);
                spec.source_id = format!("src/{object}");
                envelope.supersede_decision(object, spec)?;
                envelope.record_admission(admission(replacement))?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// Publishes one canonical-claim descriptor over `text` for `object` and returns the descriptor's object id.
    fn claim(&self, key: &str, object: &str, text: &str) -> String {
        let evidence = self.ingest(key, text.as_bytes(), false);
        self.descriptor(
            key,
            "canonical_claims",
            "decision_summary",
            &[("object_id", object)],
            &evidence,
            text,
        )
    }

    /// Publishes one descriptor over `buffer` and returns its object id.
    fn descriptor(
        &self,
        key: &str,
        class: &str,
        representation: &str,
        identity: &[(&str, &str)],
        evidence: &(String, String),
        buffer: &str,
    ) -> String {
        let mut published = None;
        self.store
            .commit(intent(&format!("descriptor-{key}")), |envelope| {
                let outcome = envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class,
                            identity,
                            revision: "1",
                            representation,
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: DOMAIN,
                        scope_id: Some(SCOPE),
                        evidence_id: &evidence.0,
                        artifact_digest: &evidence.1,
                        buffer,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap_or_else(|error| panic!("descriptor publication: {error:?}"));
                published = Some(outcome.object_id);
                Ok(String::new())
            })
            .unwrap();
        published.unwrap()
    }

    /// One page through `discovery`, asserting afterwards that discovery committed nothing.
    fn page(
        &self,
        discovery: &mut RelatedMemoryDiscovery,
        broker: &mut EvidenceBroker,
        cursor: Option<&str>,
    ) -> Result<daemon::curator::related_memories::RelatedPage, daemon::curator::broker::Refusal>
    {
        let tip = self.store.tip().unwrap();
        let page = discovery.page(
            &self.store,
            broker,
            cursor,
            &EvalBudget::unbounded(),
            self.now,
        );
        assert_eq!(self.store.tip().unwrap(), tip, "discovery writes no commit");
        page
    }
}

const SUBJECT: &str = "bun builds the workspace";

fn decision_spec(object: &str, source_revision: i64) -> DecisionSpec {
    DecisionSpec {
        decision_id: format!("decision-{object}"),
        object_id: object.to_string(),
        domain_id: DOMAIN.to_string(),
        proposition_id: None,
        scope_id: Some(SCOPE.to_string()),
        anchor_id: None,
        evidence_id: None,
        decision_kind: "architecture".to_string(),
        payload: DecisionPayload {
            summary: format!("summary {object}"),
            rationale: format!("rationale {object}"),
        },
        source_kind: "repo".to_string(),
        source_id: format!("src/{object}"),
        source_revision,
        sensitivity: Sensitivity::Normal,
    }
}

fn admission(object: &str) -> kernel::AdmissionRequest {
    kernel::AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(object.to_string()),
        source_class: Some(kernel::SourceClass::ExplicitUser),
        taint_class: Some(kernel::TaintClass::UserExplicit),
        event: kernel::AdmissionEvent {
            kind: kernel::EventKind::Other,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "test".to_string(),
        },
    }
}

fn text_of(hit: &RelatedHit) -> String {
    String::from_utf8(hit.excerpt.clone()).unwrap()
}

/// `count` admitted decisions: canonical claims that mention the subject terms except index 1, and a promoted memory holding the contradiction last. Promoted memories are walked after every canonical claim, so the contradiction is always on the final page.
fn seed(fixture: &Fixture, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let object = format!("decision-{index:02}");
            fixture.decision(&object);
            let key = format!("claim-{index:02}");
            if index == count - 1 {
                let text = format!(
                    "later page: the workspace does NOT build with bun; contradiction {index}"
                );
                let evidence = fixture.ingest(&key, text.as_bytes(), false);
                fixture.descriptor(
                    &key,
                    "promoted_memory",
                    "summary",
                    &[("decision_object_id", object.as_str())],
                    &evidence,
                    &text,
                );
            } else if index == 1 {
                fixture.claim(&key, &object, "an unrelated note about lunch");
            } else {
                fixture.claim(
                    &key,
                    &object,
                    &format!("initial context: the workspace builds with bun, claim {index}"),
                );
            }
            object
        })
        .collect()
}

/// Every hit from a fresh discovery walked to completion, ending the model batch between pages as the coordinator does.
fn drain(
    discovery: &mut RelatedMemoryDiscovery,
    fixture: &Fixture,
    broker: &mut EvidenceBroker,
) -> Vec<RelatedHit> {
    let mut cursor = None;
    let mut hits = Vec::new();
    loop {
        broker.accounting.end_batch();
        let page = fixture.page(discovery, broker, cursor.as_deref()).unwrap();
        hits.extend(page.hits);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return hits,
        }
    }
}

fn owners(broker: &EvidenceBroker) -> Vec<String> {
    broker
        .ledger
        .union()
        .members()
        .filter_map(|member| member.owner_id.clone())
        .collect()
}

#[test]
fn one_page_delivers_the_later_contradiction_and_charges_only_disclosed_excerpts() {
    let fixture = Fixture::open();
    let seeded = seed(&fixture, 4);
    let anchor = fixture.ingest("anchor", b"anchor", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&anchor.0));
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);

    let first = fixture.page(&mut discovery, &mut broker, None).unwrap();
    assert_eq!(first.completeness, Completeness::Complete);
    assert_eq!(
        first.hits.len(),
        3,
        "the unrelated note is filtered without disclosure"
    );
    assert!(first.next_cursor.is_none());
    assert!(!first.withheld);
    let contradiction = first
        .hits
        .last()
        .expect("the promoted memory is walked after every canonical claim");
    assert!(text_of(contradiction).contains("NOT build"));
    assert!(contradiction.span.end - contradiction.span.start <= MAX_EXCERPT_BYTES as u64);
    assert_eq!(contradiction.shared_origin, None);
    // Every delivered hit is a disclosed union member owned by its decision; the filtered note is not, and only disclosed excerpts are inspected or charged.
    let owners = owners(&broker);
    assert_eq!(owners.len(), 3);
    assert!(!owners.contains(&seeded[1]));
    assert!(owners.contains(&seeded[3]));
    assert_eq!(broker.accounting.issued_inspections(), 3);
    assert_eq!(
        broker.accounting.model_visible_bytes(),
        first
            .hits
            .iter()
            .map(|hit| hit.span.end - hit.span.start)
            .sum::<u64>()
    );
    assert!(broker.ledger.conclusions_usable());
    // Citing one hit cannot shrink the union.
    broker.ledger.record_citation(&first.hits[0].alias).unwrap();
    assert_eq!(broker.ledger.uncited_disclosed().count(), 2);
    assert_eq!(broker.ledger.union().members().count(), 4);
    // Aliases exist only for disclosed hits: the next alias the broker issues follows the third.
    let next = broker.aliases.issue(
        broker
            .aliases
            .resolve(first.hits[2].alias.as_str())
            .unwrap()
            .1
            .clone(),
    );
    assert_eq!(next.as_str(), "ref-4");
}

#[test]
fn pages_continue_at_one_snapshot_and_see_eligibility_changes_at_disclosure() {
    let fixture = Fixture::open();
    let seeded = seed(&fixture, MAX_RELATED_PAGE_HITS + 4);
    let anchor = fixture.ingest("anchor", b"anchor", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&anchor.0));
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);
    let first = fixture.page(&mut discovery, &mut broker, None).unwrap();
    assert_eq!(first.completeness, Completeness::PageFull);
    assert_eq!(first.hits.len(), MAX_RELATED_PAGE_HITS);
    let cursor = first.next_cursor.clone().expect("a full page continues");
    // Two related claims remain undelivered besides the contradiction. Retire one and supersede the other between pages: both changes are seen at disclosure and withheld, never reported as absence.
    let delivered = owners(&broker);
    let undelivered: Vec<&String> = seeded[..seeded.len() - 1]
        .iter()
        .enumerate()
        .filter(|(index, object)| *index != 1 && !delivered.contains(object))
        .map(|(_, object)| object)
        .collect();
    assert_eq!(undelivered.len(), 2);
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_decision(undelivered[0])?;
            Ok(String::new())
        })
        .unwrap();
    fixture.supersede(undelivered[1], "decision-replacement");
    broker.accounting.end_batch();
    let second = fixture
        .page(&mut discovery, &mut broker, Some(&cursor))
        .unwrap();
    assert_eq!(second.completeness, Completeness::Complete);
    assert!(second.withheld);
    assert_eq!(
        second.hits.len(),
        1,
        "only the contradiction is still eligible"
    );
    assert!(text_of(&second.hits[0]).contains("NOT build"));
    assert!(!owners(&broker).contains(undelivered[0]));
    assert!(!owners(&broker).contains(undelivered[1]));
    assert!(broker.ledger.conclusions_usable());
    // Cursors from another run and cursors this run never issued are refused rather than restarting discovery.
    let mut other = RelatedMemoryDiscovery::new(SUBJECT);
    assert_eq!(
        fixture
            .page(&mut other, &mut broker, Some(&cursor))
            .unwrap_err()
            .code,
        RefusalCode::InvalidCursor
    );
    for forged in ["cur-2", "cur-0", "", "1:0:decision-00"] {
        assert_eq!(
            fixture
                .page(&mut discovery, &mut broker, Some(forged))
                .unwrap_err()
                .code,
            RefusalCode::InvalidCursor
        );
    }
    // Replaying an issued cursor continues at its own snapshot: a claim admitted afterwards is invisible to it, and the replayed row is re-disclosed under a fresh alias that shares its origin.
    fixture.decision("decision-late");
    fixture.claim("late", "decision-late", "late: bun builds again");
    broker.accounting.end_batch();
    let replay = fixture
        .page(&mut discovery, &mut broker, Some(&cursor))
        .unwrap();
    assert_eq!(replay.hits.len(), 1);
    assert_eq!(replay.completeness, Completeness::Complete);
    assert_eq!(
        replay.hits[0].shared_origin.as_ref(),
        Some(&second.hits[0].alias)
    );
    let fresh = drain(&mut discovery, &fixture, &mut broker);
    assert!(fresh.iter().any(|hit| text_of(hit).contains("late:")));
}

#[test]
fn candidate_and_inspection_bounds_stop_with_explicit_incompleteness() {
    let fixture = Fixture::open();
    // More unrelated candidates than one call may examine, then one related claim.
    for index in 0..MAX_RELATED_CANDIDATES_PER_PAGE {
        let object = format!("decision-u{index:03}");
        fixture.decision(&object);
        fixture.claim(
            &format!("u{index:03}"),
            &object,
            &format!("unrelated {index}"),
        );
    }
    fixture.decision("decision-zz");
    fixture.claim("zz", "decision-zz", "zz: the workspace builds with bun");
    let anchor = fixture.ingest("anchor", b"anchor", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&anchor.0));
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);
    let first = fixture.page(&mut discovery, &mut broker, None).unwrap();
    assert_eq!(
        first.completeness,
        Completeness::CandidateBound,
        "an early stop is incompleteness, not absence"
    );
    assert!(!first.withheld);
    let cursor = first.next_cursor.clone().expect("the bound continues");
    let second = fixture
        .page(&mut discovery, &mut broker, Some(&cursor))
        .unwrap();
    assert_eq!(second.completeness, Completeness::Complete);
    assert_eq!(
        first.hits.len() + second.hits.len(),
        1,
        "keyset order follows lineage digests, so the related row lands on either page"
    );
    assert_eq!(
        broker.accounting.issued_inspections(),
        1,
        "probing unrelated candidates is neither inspected nor charged"
    );
    assert_eq!(broker.accounting.model_visible_bytes(), 33);
    assert_eq!(
        broker.buffers.loaded(),
        1,
        "only the disclosed artifact is retained"
    );
    // A run whose inspection budget is spent gets a capacity refusal at the first disclosure, wherever it falls, not a silently shorter page.
    let mut exhausted = fixture
        .broker(PROJECT, std::slice::from_ref(&anchor.0))
        .with_inspection_limit(0);
    let mut cursor = None;
    let outcome = loop {
        match fixture.page(&mut discovery, &mut exhausted, cursor.as_deref()) {
            Ok(page) if page.next_cursor.is_some() => cursor = page.next_cursor,
            other => break other,
        }
    };
    assert_eq!(outcome.unwrap_err().code, RefusalCode::InspectionLimit);
    assert_eq!(exhausted.ledger.disclosed().count(), 0);
    assert_eq!(exhausted.buffers.loaded(), 0);
}

#[test]
fn batch_headroom_ends_a_page_without_marking_the_run_partial() {
    let fixture = Fixture::open();
    seed(&fixture, MAX_RELATED_PAGE_HITS + 2);
    let anchor = fixture.ingest("anchor", b"anchor", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&anchor.0));
    // Two operations of this batch are already spent, so the page can disclose at most six hits.
    for _ in 0..2 {
        broker.accounting.admit_operation(None).unwrap();
    }
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);
    let first = fixture.page(&mut discovery, &mut broker, None).unwrap();
    assert_eq!(first.completeness, Completeness::CapacityBound);
    assert_eq!(first.hits.len(), MAX_OPERATIONS_PER_BATCH - 2);
    assert!(
        broker.ledger.conclusions_usable(),
        "stopping at the headroom is not a partial disclosure"
    );
    // With no headroom at all, the page cannot start and says so as a refusal.
    assert_eq!(
        fixture
            .page(&mut discovery, &mut broker, first.next_cursor.as_deref())
            .unwrap_err()
            .code,
        RefusalCode::BatchLimit
    );
    assert!(broker.ledger.conclusions_usable());
    broker.accounting.end_batch();
    let second = fixture
        .page(&mut discovery, &mut broker, first.next_cursor.as_deref())
        .unwrap();
    assert_eq!(
        first.hits.len() + second.hits.len(),
        MAX_RELATED_PAGE_HITS + 1,
        "the undelivered row is delivered by the next page, once"
    );
    let mut delivered = owners(&broker);
    delivered.sort();
    delivered.dedup();
    assert_eq!(
        delivered.len(),
        MAX_RELATED_PAGE_HITS + 1,
        "no decision is disclosed twice"
    );
    // A run ceiling reached after the page disclosed something still delivers those hits; the broker has marked the run partial.
    let mut capped = fixture
        .broker(PROJECT, std::slice::from_ref(&anchor.0))
        .with_inspection_limit(2);
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);
    let page = fixture.page(&mut discovery, &mut capped, None).unwrap();
    assert_eq!(page.completeness, Completeness::CapacityBound);
    assert_eq!(page.hits.len(), 2);
    assert!(page.next_cursor.is_some());
    assert!(!capped.ledger.conclusions_usable());
    assert_eq!(capped.ledger.disclosed().count(), 2);
}

#[test]
fn probe_bound_and_oversized_artifacts_are_reported_not_scanned() {
    let fixture = Fixture::open();
    let artifact = usize::try_from(MAX_PROBE_ARTIFACT_BYTES).unwrap();
    let page_budget = usize::try_from(MAX_PAGE_PROBE_BYTES).unwrap();
    let filler = |bytes: usize| "filler words ".repeat(bytes / 13);
    // Enough unrelated near-maximal artifacts to exceed one page's probe budget, one related artifact past the per-artifact bound, and one small related claim.
    let large = page_budget / artifact + 1;
    for index in 0..large {
        let object = format!("decision-h{index}");
        fixture.decision(&object);
        fixture.claim(&format!("h{index}"), &object, &filler(artifact - 64));
    }
    fixture.decision("decision-huge");
    fixture.claim(
        "huge",
        "decision-huge",
        &format!("{}the workspace", filler(artifact + 64)),
    );
    fixture.decision("decision-small");
    fixture.claim("small", "decision-small", "small: bun builds the workspace");
    let anchor = fixture.ingest("anchor", b"anchor", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&anchor.0));
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);
    let mut cursor = None;
    let mut completeness = Vec::new();
    let mut hits = Vec::new();
    let mut withheld = false;
    loop {
        broker.accounting.end_batch();
        let page = fixture
            .page(&mut discovery, &mut broker, cursor.as_deref())
            .unwrap();
        completeness.push(page.completeness);
        withheld |= page.withheld;
        hits.extend(page.hits);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "only the small claim is related and probeable"
    );
    assert!(text_of(&hits[0]).starts_with("small:"));
    assert!(
        completeness.contains(&Completeness::ProbeBound),
        "a page stopped at its probe budget: {completeness:?}"
    );
    assert_eq!(completeness.last(), Some(&Completeness::Complete));
    assert!(
        withheld,
        "the oversized artifact is reported as withheld, not scanned"
    );
    assert!(!owners(&broker).contains(&"decision-huge".to_string()));
    assert_eq!(broker.accounting.issued_inspections(), 1);
    assert_eq!(
        broker.buffers.loaded(),
        1,
        "probing retains nothing; the disclosed artifact alone is loaded"
    );
}

#[test]
fn ineligible_and_unrenderable_candidates_are_withheld_without_disclosure() {
    let fixture = Fixture::open();
    seed(&fixture, 3);
    fixture.decision_at_revision("decision-r2", 2);
    fixture.claim(
        "r2",
        "decision-r2",
        "bun builds the workspace; decision revised twice",
    );
    fixture.decision("decision-marker");
    fixture.claim(
        "marker",
        "decision-marker",
        &format!(
            "bun builds the workspace {}",
            kernel::OPERATOR_REDACTION_PLACEHOLDER
        ),
    );
    // A multibyte lead-in longer than the excerpt's lead, so the span start falls inside the lead.
    fixture.decision("decision-utf8");
    let utf8 = format!("{}the wörkspace builds with bün ✅", "é".repeat(50));
    fixture.claim("utf8", "decision-utf8", &utf8);
    let anchor = fixture.ingest("anchor", b"anchor", false);
    let mut broker = fixture.broker(PROJECT, std::slice::from_ref(&anchor.0));
    let mut discovery = RelatedMemoryDiscovery::new(SUBJECT);
    let page = fixture.page(&mut discovery, &mut broker, None).unwrap();
    assert_eq!(page.completeness, Completeness::Complete);
    assert!(
        page.withheld,
        "the render check withholds the buffer carrying a redaction placeholder"
    );
    assert_eq!(
        page.hits.len(),
        4,
        "the decision at revision two is bound at its live revision, not a guessed one"
    );
    assert!(
        page.hits
            .iter()
            .any(|hit| text_of(hit).contains("revised twice"))
    );
    assert!(
        page.hits
            .iter()
            .all(|hit| !text_of(hit).contains(kernel::OPERATOR_REDACTION_PLACEHOLDER))
    );
    // Q19: the span is half-open on UTF-8 boundaries into the artifact, and the excerpt is exactly those bytes, containing the matched term.
    let multibyte = page
        .hits
        .iter()
        .find(|hit| text_of(hit).contains("builds with bün"))
        .unwrap();
    let start = usize::try_from(multibyte.span.start).unwrap();
    let end = usize::try_from(multibyte.span.end).unwrap();
    assert!(utf8.is_char_boundary(start) && utf8.is_char_boundary(end));
    assert_eq!(multibyte.excerpt, utf8.as_bytes()[start..end]);
    assert!(
        start > 0 && start < 50 * 2,
        "the lead-in cut lands inside the multibyte prefix"
    );
    // A run bound to another project sees no eligible candidate and discloses nothing; that is withholding, not absence.
    let mut other = fixture.broker(OTHER_PROJECT, std::slice::from_ref(&anchor.0));
    let page = fixture.page(&mut discovery, &mut other, None).unwrap();
    assert_eq!(page.completeness, Completeness::Complete);
    assert!(page.hits.is_empty());
    assert!(page.withheld);
    assert_eq!(other.ledger.disclosed().count(), 0);
    assert_eq!(other.accounting.issued_inspections(), 0);
    assert_eq!(other.buffers.loaded(), 0);
    // A subject without a matcher term completes at once and probes nothing.
    let mut empty = RelatedMemoryDiscovery::new("a to be");
    let page = fixture.page(&mut empty, &mut other, None).unwrap();
    assert_eq!(page.completeness, Completeness::Complete);
    assert!(page.hits.is_empty() && !page.withheld);
}
