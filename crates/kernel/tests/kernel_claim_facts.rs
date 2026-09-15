//! Claim facts against a real store: every value the reader returns is
//! compared with what the test wrote or with an independent kernel read, never
//! with another projection of the same reader.

#![cfg(feature = "test-support")]

mod claim_fixture;

use std::num::{NonZeroU64, NonZeroUsize};

use claim_fixture::{
    DOMAIN, Fixture, MAX_DETAIL_BYTES, admission, bounds, decision, derived, direct, intent,
    request,
};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionEvent, AdmissionRequest, CausalClass, ClaimFactBounds, ClaimFactsError, EventKind,
    KernelError, MAX_CLAIM_OBJECT_ID_BYTES, Maturity, POLICY_REVISION, RepositoryProvenance,
    RepresentationExclusion, ScopeSpec, Sensitivity, ServedStanding, SourceClass,
    SourceDescriptorDetail, StagingCandidateSpec, SupportingApproval, Surface, SurfaceVisibility,
    TaintClass, UnknownReason,
};
use serde_json::Value;

/// The serving route's visibility for `object_id` at `as_of` on `surface`;
/// `Hidden` when the route lists no row.
fn served_visibility(
    fixture: &Fixture,
    object_id: &str,
    as_of: i64,
    surface: Surface,
) -> SurfaceVisibility {
    fixture
        .store
        .visible_as_of(surface, as_of)
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.object.object_id == object_id)
        .map_or(SurfaceVisibility::Hidden, |row| row.visibility)
}

fn served(facts: &kernel::ClaimFacts) -> &kernel::ServedFacts {
    match &facts.served {
        ServedStanding::Served(served) => served,
        other => panic!("expected served facts, found {other:?}"),
    }
}

fn stored_payload(fixture: &Fixture, object_id: &str) -> Vec<u8> {
    rusqlite::Connection::open(fixture.root.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [object_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn write_payload(fixture: &Fixture, object_id: &str, payload: &[u8]) {
    rusqlite::Connection::open(fixture.root.path().join("kernel.sqlite"))
        .unwrap()
        .execute(
            "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
            rusqlite::params![payload, object_id],
        )
        .unwrap();
}

fn rewrite_detail(
    fixture: &Fixture,
    object_id: &str,
    edit: impl FnOnce(&mut SourceDescriptorDetail),
) {
    let mut stored: Value = serde_json::from_slice(&stored_payload(fixture, object_id)).unwrap();
    let mut detail: SourceDescriptorDetail =
        serde_json::from_str(stored["detail"].as_str().unwrap()).unwrap();
    edit(&mut detail);
    stored["detail"] = Value::String(serde_json::to_string(&detail).unwrap());
    write_payload(fixture, object_id, &serde_json::to_vec(&stored).unwrap());
}

#[test]
fn claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot() {
    let fixture = Fixture::open();
    let (evidence_id, _) = fixture.retain("cited", "cited evidence");
    let mut written = None;
    let admitted_at = fixture
        .store
        .commit(intent("decision-1"), |envelope| {
            envelope.insert_scope(ScopeSpec {
                scope_id: "scope-1".to_string(),
                object_id: "scope-object-1".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "fixture".to_string(),
                source_id: "scope".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
                terms: Vec::new(),
            })?;
            let mut spec = decision(1, 1);
            spec.scope_id = Some("scope-1".to_string());
            spec.evidence_id = Some(evidence_id.clone());
            envelope.insert_decision(spec)?;
            let mut request = admission("decision-object-1");
            request.event.evidence_id = Some(evidence_id.clone());
            written = Some(envelope.record_admission(request)?);
            Ok(String::new())
        })
        .unwrap()
        .commit_seq;
    let written = written.unwrap();
    let summary = fixture.publish_summary(1, 1);
    let promoted = fixture.publish("promoted_memory", "summary", 1, 1, "decision 1");
    let fenced = fixture.tip();

    let facts = fixture.facts("decision-object-1", fenced);
    assert_eq!(facts.object.object_kind, "decision");
    assert_eq!(facts.object.source_revision, 1);
    assert_eq!(facts.object.created_commit_seq, admitted_at);
    assert_eq!(facts.object.invalidated_commit_seq, None);
    assert_eq!(facts.decision.decision_id, "decision-1");
    assert_eq!(facts.decision.decision_kind, "architecture");
    assert_eq!(facts.decision.scope_id.as_deref(), Some("scope-1"));
    assert_eq!(
        facts.decision.evidence_id.as_deref(),
        Some(evidence_id.as_str())
    );
    assert_eq!(facts.decision.proposition_id, None);
    assert_eq!(facts.decision.anchor_id, None);

    let own = facts.own_admission.as_ref().expect("own admission row");
    assert_eq!(own.admission_decision_id, written.admission_decision_id);
    assert_eq!(own.historical_maturity, written.historical_maturity);
    assert_eq!(own.effective_maturity, written.effective_maturity);
    assert_eq!(own.disposition, written.disposition);
    assert_eq!(own.visibility, written.visibility);
    assert_eq!(own.outcome, written.outcome);
    assert_eq!(own.sensitivity, written.sensitivity);
    assert_eq!(own.source_class, SourceClass::TrustedLocalCode);
    assert_eq!(own.taint_class, TaintClass::CurrentCode);
    assert_eq!(own.event_kind, EventKind::Other);
    assert_eq!(own.policy_revision, POLICY_REVISION);
    assert_eq!(own.evidence_id.as_deref(), Some(evidence_id.as_str()));
    assert_eq!(own.trigger_object_id, None);
    assert_eq!(own.supporting_approval, None);
    assert!(!own.elevated_support);
    assert_eq!(own.commit_seq, admitted_at);
    assert!(facts.lineage_admission.is_none());

    // The served interpretation is the serving route's own answer on every surface.
    let served = served(&facts);
    for (surface, reported) in [
        (Surface::AutoInject, served.auto_inject),
        (Surface::AutoSearch, served.auto_search),
        (Surface::ExplicitSearch, served.explicit_search),
    ] {
        assert_eq!(
            reported,
            served_visibility(&fixture, "decision-object-1", fenced, surface),
            "{surface:?}"
        );
    }
    assert_eq!(served.explicit_search, SurfaceVisibility::Labeled);
    assert_eq!(served.sensitivity, Sensitivity::Normal);

    // Occurrence inventory: two published representations, one explicit exclusion.
    assert_eq!(facts.occurrences.len(), 2);
    for (occurrence, published, class, representation) in [
        (
            &facts.occurrences[0],
            &summary,
            OccurrenceClass::CanonicalClaims,
            "decision_summary",
        ),
        (
            &facts.occurrences[1],
            &promoted,
            OccurrenceClass::PromotedMemory,
            "summary",
        ),
    ] {
        assert_eq!(occurrence.class, class);
        assert_eq!(occurrence.representation, representation);
        assert_eq!(occurrence.occurrence_id, published.occurrence_id);
        assert_eq!(occurrence.lineage_id, published.lineage_id);
        assert_eq!(occurrence.payload_id, published.payload_id);
        assert_eq!(
            occurrence.descriptor_object_id,
            published.descriptor_object_id
        );
        assert_eq!(occurrence.artifact_digest, published.digest);
        assert_eq!(occurrence.evidence_id, published.evidence_id);
        assert_eq!(occurrence.descriptor_commit_seq, published.commit_seq);
    }
    assert_ne!(
        facts.occurrences[0].occurrence_id, facts.occurrences[1].occurrence_id,
        "equal bytes under two classes stay two occurrences"
    );
    let excluded: Vec<(OccurrenceClass, &str, RepresentationExclusion)> = facts
        .excluded_representations
        .iter()
        .map(|entry| (entry.class, entry.representation, entry.reason))
        .collect();
    assert_eq!(
        excluded,
        [(
            OccurrenceClass::CanonicalClaims,
            "rationale",
            RepresentationExclusion::NoDescriptor
        )]
    );
    assert_eq!(
        facts.causality,
        CausalClass::Unknown(UnknownReason::NoRecord)
    );
    assert_eq!(facts.causal_record, None);

    // Later writes do not change what the fenced snapshot says.
    fixture.admit_decision(2, 1);
    assert_eq!(fixture.facts("decision-object-1", fenced), facts);

    // Before the decision existed the id is missing, not an error.
    let before = fixture
        .store
        .claim_facts_as_of(
            &["decision-object-1".to_string()],
            admitted_at - 1,
            bounds(),
        )
        .unwrap();
    assert!(before.claims.is_empty());
    assert_eq!(before.missing, ["decision-object-1"]);
    assert_eq!(before.known_as_of, admitted_at - 1);
}

#[test]
fn a_lineage_admission_binds_every_object_on_the_lineage() {
    let fixture = Fixture::open();
    let (own, _) = fixture.admit_decision(1, 1);
    let before = fixture.tip();
    let now: i64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap();
    fixture
        .store
        .stage_candidate(StagingCandidateSpec {
            extraction_run_id: "run-lineage".to_string(),
            candidate_id: "candidate-lineage".to_string(),
            extractor: "fixture".to_string(),
            source_kind: "fixture".to_string(),
            source_id: "claim-lineage".to_string(),
            source_revision: 1,
            candidate_kind: "domain".to_string(),
            payload: "name-lineage".to_string(),
            provenance: Some(RepositoryProvenance {
                repository_id: "repo".to_string(),
                revision: "abc123".to_string(),
            }),
            recorded_at: now,
            lease_expires_at: now + 60_000,
        })
        .unwrap();
    let mut written = None;
    fixture
        .store
        .commit(intent("lineage-quarantine"), |envelope| {
            written = Some(envelope.record_admission(AdmissionRequest {
                candidate_id: Some("candidate-lineage".to_string()),
                subject_object_id: None,
                source_class: Some(SourceClass::TrustedLocalCode),
                taint_class: Some(TaintClass::CurrentCode),
                event: AdmissionEvent {
                    kind: EventKind::Quarantine,
                    trigger_object_id: None,
                    approval_object_id: None,
                    evidence_id: None,
                    reason: "lineage quarantined".to_string(),
                },
            })?);
            Ok(String::new())
        })
        .unwrap();
    let written = written.unwrap();
    let quarantined_at = fixture.tip();

    let facts = fixture.facts("decision-object-1", quarantined_at);
    let lineage = facts.lineage_admission.as_ref().expect("lineage row");
    assert_eq!(lineage.admission_decision_id, written.admission_decision_id);
    assert_eq!(lineage.disposition, written.disposition);
    assert_eq!(lineage.event_kind, EventKind::Quarantine);
    assert_eq!(lineage.policy_revision, POLICY_REVISION);
    assert_eq!(
        facts.own_admission.as_ref().unwrap().admission_decision_id,
        own.admission_decision_id,
        "the own row is unchanged by a lineage decision"
    );
    // The serving view folds the lineage row into every surface.
    let served = served(&facts);
    for (surface, reported) in [
        (Surface::AutoInject, served.auto_inject),
        (Surface::AutoSearch, served.auto_search),
        (Surface::ExplicitSearch, served.explicit_search),
    ] {
        assert_eq!(
            reported,
            served_visibility(&fixture, "decision-object-1", quarantined_at, surface),
            "{surface:?}"
        );
    }
    assert_eq!(served.explicit_search, SurfaceVisibility::Hidden);
    assert!(
        fixture
            .facts("decision-object-1", before)
            .lineage_admission
            .is_none()
    );
}

#[test]
fn causality_in_facts_equals_the_causal_reader_at_every_snapshot() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    fixture
        .record(
            "direct",
            request("decision-object-1", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    let direct_at = fixture.tip();
    fixture
        .record(
            "derived",
            request("decision-object-1", 1, derived(&[("domain-object", 1)])),
        )
        .unwrap();
    let derived_at = fixture.tip();
    for snapshot in [direct_at - 1, direct_at, derived_at] {
        let facts = fixture.facts("decision-object-1", snapshot);
        let reading = fixture.reading("decision-object-1", snapshot);
        assert_eq!(facts.causality, reading.class, "snapshot {snapshot}");
        assert_eq!(facts.causal_record, reading.record, "snapshot {snapshot}");
    }
    assert_eq!(
        fixture.facts("decision-object-1", direct_at).causality,
        CausalClass::DirectObservation {
            acquisition_evidence_id: evidence_id,
            artifact_digest: digest,
        }
    );
}

#[test]
fn supporting_approval_is_copied_with_its_validity_at_the_snapshot() {
    let fixture = Fixture::open();
    fixture.seed_approval();
    let mut approved = admission("decision-object-1");
    approved.source_class = Some(SourceClass::ModelInference);
    approved.taint_class = Some(TaintClass::AssistantInference);
    approved.event = AdmissionEvent {
        kind: EventKind::Verify,
        trigger_object_id: None,
        approval_object_id: Some("approval".to_string()),
        evidence_id: None,
        reason: "verified under approval".to_string(),
    };
    let mut written = None;
    fixture
        .store
        .commit(intent("approved-decision"), |envelope| {
            envelope.insert_decision(decision(1, 1))?;
            written = Some(envelope.record_admission(approved)?);
            Ok(String::new())
        })
        .unwrap();
    let written = written.unwrap();
    let approved_at = fixture.tip();
    let facts = fixture.facts("decision-object-1", approved_at);
    let own = facts.own_admission.as_ref().unwrap();
    assert_eq!(own.effective_maturity, written.effective_maturity);
    assert_eq!(own.effective_maturity, Maturity::Verified);
    assert!(own.elevated_support);
    assert_eq!(
        own.supporting_approval,
        Some(SupportingApproval {
            object_id: "approval".to_string(),
            valid_at_snapshot: true,
        })
    );

    let mut demoted = None;
    fixture
        .store
        .commit(intent("revoke"), |envelope| {
            demoted = Some(envelope.revoke_approval("approval", "authority revoked")?);
            Ok(String::new())
        })
        .unwrap();
    let demoted = demoted.unwrap();
    assert_eq!(demoted.len(), 1);
    let revoked_at = fixture.tip();

    // The demotion row is copied as written; the earlier snapshot still reads
    // the earlier row with the approval valid as it was then.
    let after = fixture.facts("decision-object-1", revoked_at);
    let own = after.own_admission.as_ref().unwrap();
    assert_eq!(own.admission_decision_id, demoted[0].admission_decision_id);
    assert_eq!(own.effective_maturity, demoted[0].effective_maturity);
    assert_eq!(own.effective_maturity, Maturity::Candidate);
    assert_eq!(own.historical_maturity, Maturity::Verified);
    assert!(!own.elevated_support);
    assert_eq!(fixture.facts("decision-object-1", approved_at), facts);

    // A row still naming the approval reads it as invalid once the approval is
    // revoked at the snapshot.
    let latest_own = own.admission_decision_id.clone();
    fixture.sql(&format!(
        "UPDATE admission_decisions SET approval_object_id='approval'
         WHERE admission_decision_id='{latest_own}';"
    ));
    let rewritten = fixture.facts("decision-object-1", revoked_at);
    assert_eq!(
        rewritten.own_admission.unwrap().supporting_approval,
        Some(SupportingApproval {
            object_id: "approval".to_string(),
            valid_at_snapshot: false,
        })
    );
}

#[test]
fn bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    fixture
        .record(
            "direct",
            request("decision-object-1", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    let tip = fixture.tip();

    let tight = ClaimFactBounds {
        max_claims: NonZeroUsize::new(1).unwrap(),
        max_causal_payload_bytes: NonZeroU64::new(8).unwrap(),
    };
    let snapshot = fixture
        .store
        .claim_facts_as_of(&["decision-object-1".to_string()], tip, tight)
        .unwrap();
    assert_eq!(
        snapshot.claims[0].causality,
        CausalClass::Unknown(UnknownReason::Oversized)
    );
    let identity = snapshot.claims[0].causal_record.as_ref().unwrap();
    let full = fixture
        .store
        .causal_class_as_of("decision-object-1", tip, MAX_DETAIL_BYTES)
        .unwrap()
        .record
        .unwrap();
    assert_eq!(identity.object_id, full.object_id);
    assert_eq!(identity.operation, None, "the detail was not decoded");
    assert!(full.operation.is_some());
    assert_eq!(
        fixture.store.claim_facts_as_of(
            &["decision-object-1".to_string(), "domain-object".to_string()],
            tip,
            tight
        ),
        Err(ClaimFactsError::TooManyClaims)
    );
    assert_eq!(
        fixture.store.claim_facts_as_of(
            &[
                "decision-object-1".to_string(),
                "decision-object-1".to_string()
            ],
            tip,
            bounds()
        ),
        Err(ClaimFactsError::DuplicateClaim)
    );
    assert_eq!(
        fixture
            .store
            .claim_facts_as_of(&["domain-object".to_string()], tip, bounds()),
        Err(ClaimFactsError::NotADecision)
    );
    assert_eq!(
        fixture
            .store
            .claim_facts_as_of(&["decision-object-1".to_string()], tip + 1, bounds()),
        Err(ClaimFactsError::Kernel(KernelError::FutureSnapshot))
    );
    let empty = fixture.store.claim_facts_as_of(&[], tip, bounds()).unwrap();
    assert!(empty.claims.is_empty() && empty.missing.is_empty());

    // Each required column that fails to decode is an explicit error.
    let read = || {
        fixture
            .store
            .claim_facts_as_of(&["decision-object-1".to_string()], tip, bounds())
    };
    fixture.sql("UPDATE admission_decisions SET sensitivity_class='bogus';");
    assert_eq!(read(), Err(ClaimFactsError::MalformedRequiredField));
    fixture.sql("UPDATE admission_decisions SET sensitivity_class='normal', disposition='bogus';");
    assert_eq!(read(), Err(ClaimFactsError::MalformedRequiredField));
    fixture.sql("UPDATE admission_decisions SET disposition='active', maturity='bogus';");
    assert_eq!(read(), Err(ClaimFactsError::MalformedRequiredField));
    fixture.sql("UPDATE admission_decisions SET maturity='candidate';");
    assert!(read().is_ok());
    // A decision row that disagrees with its registry row is corruption.
    fixture.sql(
        "UPDATE decisions SET sensitivity_class='secret' WHERE object_id='decision-object-1';",
    );
    assert_eq!(
        read(),
        Err(ClaimFactsError::Kernel(KernelError::CorruptCanonicalRow))
    );
    fixture.sql(
        "UPDATE decisions SET sensitivity_class='normal', invalidated_commit_seq=created_commit_seq+1
         WHERE object_id='decision-object-1';",
    );
    assert_eq!(
        read(),
        Err(ClaimFactsError::Kernel(KernelError::CorruptCanonicalRow)),
        "decision liveness disagrees with the registry row"
    );
    fixture.sql(
        "UPDATE decisions SET invalidated_commit_seq=NULL WHERE object_id='decision-object-1';",
    );
    assert!(read().is_ok());

    // `claim_facts_as_of` validates each identifier before acquiring a reader, including missing identifiers.
    for id in [String::new(), "x".repeat(MAX_CLAIM_OBJECT_ID_BYTES + 1)] {
        assert_eq!(
            fixture.store.claim_facts_as_of(&[id], tip, bounds()),
            Err(ClaimFactsError::Kernel(KernelError::InvalidInput))
        );
    }
    let longest = ["x".repeat(MAX_CLAIM_OBJECT_ID_BYTES)];
    assert_eq!(
        fixture
            .store
            .claim_facts_as_of(&longest, tip, bounds())
            .unwrap()
            .missing,
        longest
    );
}

#[test]
fn corrected_claims_report_succession_and_serving_standing_at_the_snapshot() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    fixture.publish_summary(1, 1);
    fixture
        .store
        .commit(intent("correct"), |envelope| {
            envelope.correct_decision("decision-object-1", decision(2, 2))?;
            Ok(String::new())
        })
        .unwrap();
    let corrected_at = fixture.tip();
    let snapshot = fixture
        .store
        .claim_facts_as_of(
            &[
                "decision-object-1".to_string(),
                "decision-object-2".to_string(),
            ],
            corrected_at,
            bounds(),
        )
        .unwrap();
    assert_eq!(snapshot.claims.len(), 2);
    let predecessor = &snapshot.claims[0];
    assert_eq!(
        predecessor.object.invalidated_commit_seq,
        Some(corrected_at)
    );
    assert_eq!(
        predecessor.object.superseded_by.as_deref(),
        Some("decision-object-2")
    );
    assert_eq!(predecessor.object.source_revision, 1);
    assert_eq!(predecessor.served, ServedStanding::NotLiveAtSnapshot);
    assert!(predecessor.own_admission.is_some());
    // The kernel does not retire descriptors with their decision; the claim
    // materializer does, through the outbox. Until it runs, the reader reports
    // the descriptor as live, which is what the store holds.
    assert_eq!(predecessor.occurrences.len(), 1);
    let successor = &snapshot.claims[1];
    assert_eq!(successor.object.invalidated_commit_seq, None);
    assert_eq!(successor.object.source_revision, 2);
    assert_eq!(successor.served, ServedStanding::NeverAdmitted);
    assert!(successor.own_admission.is_none());
    assert_eq!(successor.excluded_representations.len(), 3);
    for surface in [
        Surface::AutoInject,
        Surface::AutoSearch,
        Surface::ExplicitSearch,
    ] {
        for object_id in ["decision-object-1", "decision-object-2"] {
            assert_eq!(
                served_visibility(&fixture, object_id, corrected_at, surface),
                SurfaceVisibility::Hidden
            );
        }
    }

    // The predecessor's snapshot before the correction shows it live and served.
    let live = fixture.facts("decision-object-1", corrected_at - 1);
    assert_eq!(live.object.invalidated_commit_seq, None);
    assert_eq!(live.object.superseded_by, None);
    assert_eq!(live.occurrences.len(), 1);
    assert_eq!(
        served(&live).explicit_search,
        served_visibility(
            &fixture,
            "decision-object-1",
            corrected_at - 1,
            Surface::ExplicitSearch
        )
    );
}

#[test]
fn facts_survive_reopen() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    fixture.publish_summary(1, 1);
    let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
    fixture
        .record(
            "direct",
            request("decision-object-1", 1, direct(&evidence_id, &digest)),
        )
        .unwrap();
    let tip = fixture.tip();
    let before = fixture.facts("decision-object-1", tip);
    let fixture = fixture.reopen();
    let after = fixture.facts("decision-object-1", tip);
    assert_eq!(after, before);
    assert!(matches!(
        after.causality,
        CausalClass::DirectObservation { .. }
    ));
    assert_eq!(after.occurrences.len(), 1);
    assert_eq!(after.object.domain_id, DOMAIN);
}

#[test]
fn a_partial_span_publication_is_outside_the_whole_buffer_inventory() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let partial = fixture.publish_span(
        "canonical_claims",
        "decision_summary",
        1,
        1,
        "decision 1",
        Some((0, 8)),
    );
    let tip = fixture.tip();
    let facts = fixture.facts("decision-object-1", tip);
    assert!(facts.occurrences.is_empty());
    assert!(
        facts
            .excluded_representations
            .contains(&kernel::ExcludedRepresentation {
                class: OccurrenceClass::CanonicalClaims,
                representation: "decision_summary",
                reason: RepresentationExclusion::NoDescriptor,
            })
    );

    let whole = fixture.publish_summary(1, 1);
    assert_ne!(whole.lineage_id, partial.lineage_id);
    let facts = fixture.facts("decision-object-1", fixture.tip());
    assert_eq!(facts.occurrences.len(), 1);
    assert_eq!(facts.occurrences[0].occurrence_id, whole.occurrence_id);
    assert_eq!(facts.occurrences[0].payload_id, whole.digest);
}

#[test]
fn served_facts_follow_the_cited_evidence_class_read_today() {
    let fixture = Fixture::open();
    let (evidence_id, _) = fixture.retain("later-secret", "normal now, secret later");
    fixture
        .store
        .commit(intent("decision-1"), |envelope| {
            let mut spec = decision(1, 1);
            spec.evidence_id = Some(evidence_id.clone());
            envelope.insert_decision(spec)?;
            envelope.record_admission(admission("decision-object-1"))?;
            Ok(String::new())
        })
        .unwrap();
    fixture.publish_summary(1, 1);
    let fenced = fixture.tip();
    let before = fixture.facts("decision-object-1", fenced);
    assert_eq!(served(&before).sensitivity, Sensitivity::Normal);
    assert_eq!(served(&before).explicit_search, SurfaceVisibility::Labeled);

    // Reasserting the same bytes as secret rewrites the evidence row in place,
    // so a reread of the older snapshot sees today's class.
    fixture.retain_as(
        "later-secret",
        "normal now, secret later",
        Sensitivity::Secret,
    );
    let after = fixture.facts("decision-object-1", fenced);
    assert_eq!(
        served(&after).sensitivity,
        Sensitivity::Secret,
        "served facts are not fixed by the snapshot"
    );
    assert_eq!(
        served(&after).explicit_search,
        served_visibility(
            &fixture,
            "decision-object-1",
            fenced,
            Surface::ExplicitSearch
        )
    );
    assert_ne!(after.served, before.served);
    // The revisioned rows are unchanged.
    assert_eq!(after.object, before.object);
    assert_eq!(after.decision, before.decision);
    assert_eq!(after.own_admission, before.own_admission);
    assert_eq!(after.lineage_admission, before.lineage_admission);
    assert_eq!(after.occurrences, before.occurrences);
    assert_eq!(
        after.excluded_representations,
        before.excluded_representations
    );
    assert_eq!(after.causality, before.causality);
    assert_eq!(after.causal_record, before.causal_record);
}

#[test]
fn occurrence_facts_refuse_a_detail_that_disagrees_with_its_guarded_rows() {
    let fixture = Fixture::open();
    fixture.admit_decision(1, 1);
    let published = fixture.publish_summary(1, 1);
    let (other_evidence, other_digest) = fixture.retain("elsewhere", "other bytes");
    let tip = fixture.tip();
    let original = stored_payload(&fixture, &published.descriptor_object_id);
    let read = || {
        fixture
            .store
            .claim_facts_as_of(&["decision-object-1".to_string()], tip, bounds())
    };
    assert_eq!(read().unwrap().claims[0].occurrences.len(), 1);

    // Each field the occurrence reports must agree with the column the
    // liveness query joined on; a detail that names another evidence row,
    // digest, payload, or lineage is corruption, not a fact.
    type Edit = fn(&mut SourceDescriptorDetail, &str, &str);
    let edits: [(&str, Edit); 4] = [
        ("evidence_id", |detail, evidence, _| {
            detail.evidence_id = evidence.to_string();
        }),
        ("artifact_digest", |detail, _, digest| {
            detail.artifact_digest = digest.to_string();
        }),
        ("payload_id", |detail, _, digest| {
            detail.payload_id = digest.to_string();
        }),
        ("lineage_id", |detail, _, _| {
            detail.lineage_id = "srclin:other".to_string();
        }),
    ];
    for (field, edit) in edits {
        rewrite_detail(&fixture, &published.descriptor_object_id, |detail| {
            edit(detail, &other_evidence, &other_digest);
        });
        assert_eq!(
            read(),
            Err(ClaimFactsError::Kernel(KernelError::CorruptCanonicalRow)),
            "{field}"
        );
        write_payload(&fixture, &published.descriptor_object_id, &original);
    }
    let scope = format!(" WHERE object_id='{}'", published.descriptor_object_id);
    fixture.sql("DROP TRIGGER object_registry_append_only_update;");
    for (field, tamper, restore) in [
        (
            "observation sensitivity",
            "UPDATE observations SET sensitivity_class='secret'",
            "UPDATE observations SET sensitivity_class='normal'",
        ),
        (
            "registry source_revision",
            "UPDATE object_registry SET source_revision=2",
            "UPDATE object_registry SET source_revision=1",
        ),
        (
            "registry source_kind",
            "UPDATE object_registry SET source_kind='promoted_memory'",
            "UPDATE object_registry SET source_kind='canonical_claims'",
        ),
    ] {
        fixture.sql(&format!("{tamper}{scope};"));
        assert_eq!(
            read(),
            Err(ClaimFactsError::Kernel(KernelError::CorruptCanonicalRow)),
            "{field}"
        );
        fixture.sql(&format!("{restore}{scope};"));
    }
    let restored = read().unwrap();
    assert_eq!(restored.claims[0].occurrences.len(), 1);
    assert_eq!(
        restored.claims[0].occurrences[0].evidence_id,
        published.evidence_id
    );
}

#[test]
fn a_registry_class_this_build_cannot_read_is_an_error_not_a_secret_default() {
    let fixture = Fixture::open();
    // Two decision objects whose registry class is unreadable: one whose
    // decision row says `secret`, the class an unreadable value would default
    // to, and one whose decision row repeats the unreadable value.
    fixture.sql(&format!(
        "PRAGMA foreign_keys=ON;
         INSERT INTO object_registry(
             object_id,object_kind,domain_id,source_kind,source_id,source_revision,
             created_commit_seq,sensitivity_class
         ) VALUES
             ('odd-object-1','decision','{DOMAIN}','fixture','odd-1',1,1,'bogus'),
             ('odd-object-2','decision','{DOMAIN}','fixture','odd-2',1,1,'bogus');
         INSERT INTO decisions(
             decision_id,object_id,decision_kind,decision_payload,created_commit_seq,
             sensitivity_class
         ) VALUES
             ('odd-1','odd-object-1','architecture',X'7b7d',1,'secret'),
             ('odd-2','odd-object-2','architecture',X'7b7d',1,'bogus');"
    ));
    let tip = fixture.tip();
    assert_eq!(
        fixture
            .store
            .claim_facts_as_of(&["odd-object-1".to_string()], tip, bounds()),
        Err(ClaimFactsError::Kernel(KernelError::CorruptCanonicalRow))
    );
    assert_eq!(
        fixture
            .store
            .claim_facts_as_of(&["odd-object-2".to_string()], tip, bounds()),
        Err(ClaimFactsError::MalformedRequiredField)
    );
}
