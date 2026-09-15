//! The candidate state table, exhausted by hand: every combination of the
//! canonical facts `classify` reads, with the expected state written out from
//! the documented precedence rather than derived by the function under test.

use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionFacts, CausalClass, ClaimDecisionFacts, ClaimFacts, ClaimOccurrence, Disposition,
    EventKind, Maturity, ObjectRow, Outcome, Sensitivity, ServedFacts, ServedStanding, SourceClass,
    SurfaceVisibility, TaintClass, UnknownReason, VisibilityRow,
};
use retrieval::claims::{
    CandidateState, ClaimCandidate, ClaimCandidateBatch, ClaimCandidateRow, classify,
};

fn occurrence(revision: i64) -> ClaimCandidateRow {
    ClaimCandidateRow {
        occurrence_id: "occ".to_string(),
        class: OccurrenceClass::CanonicalClaims,
        representation: "decision_summary".to_string(),
        object_id: "decision-object-1".to_string(),
        revision,
        artifact_digest: "0".repeat(64),
    }
}

fn admission(disposition: Disposition) -> AdmissionFacts {
    AdmissionFacts {
        admission_decision_id: "3:1".to_string(),
        commit_seq: 3,
        event_kind: EventKind::Other,
        historical_maturity: Maturity::Candidate,
        effective_maturity: Maturity::Candidate,
        disposition,
        visibility: VisibilityRow::ExplicitLabeled,
        outcome: Outcome::Admit,
        source_class: SourceClass::TrustedLocalCode,
        taint_class: TaintClass::CurrentCode,
        policy_revision: 1,
        sensitivity: Sensitivity::Normal,
        evidence_id: None,
        supporting_approval: None,
        trigger_object_id: None,
        elevated_support: false,
    }
}

fn served(explicit_search: SurfaceVisibility) -> ServedStanding {
    ServedStanding::Served(ServedFacts {
        sensitivity: Sensitivity::Normal,
        auto_inject: SurfaceVisibility::Hidden,
        auto_search: SurfaceVisibility::Hidden,
        explicit_search,
    })
}

struct Facts {
    invalidated: Option<i64>,
    superseded_by: Option<&'static str>,
    revision: i64,
    disposition: Option<Disposition>,
    lineage: Option<Disposition>,
    served: ServedStanding,
    /// Whether the kernel's live descriptor inventory lists the row `occurrence` builds.
    listed: bool,
    /// The artifact the listed descriptor names.
    listed_digest: String,
    causality: CausalClass,
}

impl Facts {
    fn current() -> Self {
        Self {
            invalidated: None,
            superseded_by: None,
            revision: 1,
            disposition: Some(Disposition::Active),
            lineage: None,
            served: served(SurfaceVisibility::Labeled),
            listed: true,
            listed_digest: "0".repeat(64),
            causality: CausalClass::Unknown(UnknownReason::NoRecord),
        }
    }

    fn build(self) -> ClaimFacts {
        ClaimFacts {
            object: ObjectRow {
                object_id: "decision-object-1".to_string(),
                object_kind: "decision".to_string(),
                domain_id: "domain".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "lineage".to_string(),
                source_revision: self.revision,
                created_commit_seq: 3,
                invalidated_commit_seq: self.invalidated,
                superseded_by: self.superseded_by.map(str::to_string),
                sensitivity: Sensitivity::Normal,
            },
            decision: ClaimDecisionFacts {
                decision_id: "decision-1".to_string(),
                decision_kind: "architecture".to_string(),
                proposition_id: None,
                scope_id: None,
                anchor_id: None,
                evidence_id: None,
            },
            own_admission: self.disposition.map(admission),
            lineage_admission: self.lineage.map(admission),
            served: self.served,
            occurrences: if self.listed {
                vec![ClaimOccurrence {
                    class: OccurrenceClass::CanonicalClaims,
                    representation: "decision_summary",
                    descriptor_object_id: "descriptor".to_string(),
                    occurrence_id: "occ".to_string(),
                    lineage_id: "lineage".to_string(),
                    payload_id: "payload".to_string(),
                    artifact_digest: self.listed_digest,
                    evidence_id: "evidence".to_string(),
                    descriptor_commit_seq: 3,
                }]
            } else {
                Vec::new()
            },
            excluded_representations: Vec::new(),
            causality: self.causality,
            causal_record: None,
        }
    }
}

#[test]
fn state_follows_the_documented_precedence() {
    let direct = CausalClass::DirectObservation {
        acquisition_evidence_id: "evidence".to_string(),
        artifact_digest: "0".repeat(64),
    };
    let cases: Vec<(&str, Option<Facts>, CandidateState)> = vec![
        ("no registry row", None, CandidateState::Retracted),
        (
            "invalidated without successor",
            Some(Facts {
                invalidated: Some(9),
                ..Facts::current()
            }),
            CandidateState::Retracted,
        ),
        (
            "invalidated with successor",
            Some(Facts {
                invalidated: Some(9),
                superseded_by: Some("decision-object-2"),
                ..Facts::current()
            }),
            CandidateState::Superseded,
        ),
        (
            "successor recorded without invalidation",
            Some(Facts {
                superseded_by: Some("decision-object-2"),
                ..Facts::current()
            }),
            CandidateState::Superseded,
        ),
        (
            "superseded disposition on a live object",
            Some(Facts {
                disposition: Some(Disposition::Superseded),
                ..Facts::current()
            }),
            CandidateState::Superseded,
        ),
        (
            "occurrence revision behind the object",
            Some(Facts {
                revision: 2,
                ..Facts::current()
            }),
            CandidateState::Stale,
        ),
        (
            "stale disposition",
            Some(Facts {
                disposition: Some(Disposition::Stale),
                ..Facts::current()
            }),
            CandidateState::Stale,
        ),
        (
            "disputed serves labeled and stays current",
            Some(Facts {
                disposition: Some(Disposition::Disputed),
                ..Facts::current()
            }),
            CandidateState::Current,
        ),
        (
            "rejected is hidden whatever the served row says",
            Some(Facts {
                disposition: Some(Disposition::Rejected),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "contradicted is hidden whatever the served row says",
            Some(Facts {
                disposition: Some(Disposition::Contradicted),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "hidden on the widest surface",
            Some(Facts {
                served: served(SurfaceVisibility::Hidden),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "never admitted",
            Some(Facts {
                disposition: None,
                served: ServedStanding::NeverAdmitted,
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "quarantined",
            Some(Facts {
                disposition: Some(Disposition::Quarantined),
                served: served(SurfaceVisibility::Hidden),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "invalidated wins over stale",
            Some(Facts {
                invalidated: Some(9),
                revision: 2,
                ..Facts::current()
            }),
            CandidateState::Retracted,
        ),
        (
            "superseded wins over stale and hidden",
            Some(Facts {
                revision: 2,
                disposition: Some(Disposition::Superseded),
                served: served(SurfaceVisibility::Hidden),
                ..Facts::current()
            }),
            CandidateState::Superseded,
        ),
        (
            "hidden serving wins over stale revision",
            Some(Facts {
                revision: 2,
                served: served(SurfaceVisibility::Hidden),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "hidden serving wins over stale disposition",
            Some(Facts {
                disposition: Some(Disposition::Stale),
                served: served(SurfaceVisibility::Hidden),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "rejected admission wins over stale revision",
            Some(Facts {
                revision: 2,
                disposition: Some(Disposition::Rejected),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "contradicted admission wins over stale revision",
            Some(Facts {
                revision: 2,
                disposition: Some(Disposition::Contradicted),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "quarantined admission wins over stale revision",
            Some(Facts {
                revision: 2,
                disposition: Some(Disposition::Quarantined),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
        (
            "listed with another artifact is stale",
            Some(Facts {
                listed_digest: "1".repeat(64),
                ..Facts::current()
            }),
            CandidateState::Stale,
        ),
        ("current", Some(Facts::current()), CandidateState::Current),
        (
            "current with direct observation",
            Some(Facts {
                causality: direct.clone(),
                ..Facts::current()
            }),
            CandidateState::Current,
        ),
        (
            "hidden with direct observation",
            Some(Facts {
                causality: direct,
                served: served(SurfaceVisibility::Hidden),
                ..Facts::current()
            }),
            CandidateState::Hidden,
        ),
    ];
    let mismatches: Vec<String> = cases
        .into_iter()
        .filter_map(|(name, facts, expected)| {
            let facts = facts.map(Facts::build);
            let actual = classify(&occurrence(1), facts.as_ref());
            (actual != expected).then(|| format!("{name}: expected {expected:?}, got {actual:?}"))
        })
        .collect();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// Unknown neutrality: for every state, the class is the only field that
/// differs between an otherwise identical genuine and unknown claim.
/// The serving view folds a lineage decision into every surface, so the
/// classifier reads the lineage row's disposition with the same precedence as
/// the own row's.
#[test]
fn a_lineage_disposition_binds_like_the_own_row() {
    let cases = [
        (Disposition::Stale, CandidateState::Stale),
        (Disposition::Superseded, CandidateState::Superseded),
        (Disposition::Rejected, CandidateState::Hidden),
        (Disposition::Contradicted, CandidateState::Hidden),
        (Disposition::Quarantined, CandidateState::Hidden),
        (Disposition::Disputed, CandidateState::Current),
        (Disposition::Active, CandidateState::Current),
    ];
    for (lineage, expected) in cases {
        let facts = Facts {
            lineage: Some(lineage),
            ..Facts::current()
        }
        .build();
        assert_eq!(
            classify(&occurrence(1), Some(&facts)),
            expected,
            "lineage {lineage:?} over an active own row"
        );
    }
    // The own row's disposition does not shadow a more restrictive lineage row.
    let facts = Facts {
        disposition: Some(Disposition::Rejected),
        lineage: Some(Disposition::Superseded),
        ..Facts::current()
    }
    .build();
    assert_eq!(
        classify(&occurrence(1), Some(&facts)),
        CandidateState::Superseded
    );
    let facts = Facts {
        disposition: Some(Disposition::Stale),
        lineage: Some(Disposition::Quarantined),
        served: served(SurfaceVisibility::Hidden),
        ..Facts::current()
    }
    .build();
    assert_eq!(
        classify(&occurrence(1), Some(&facts)),
        CandidateState::Hidden
    );
}

/// A row whose descriptor the kernel no longer lists as live, for example after
/// the artifact behind its evidence was deleted while the decision stayed
/// active, is Retracted: catch-up will tombstone it, and a lagging projection
/// must not serve it first.
#[test]
fn a_row_outside_the_live_descriptor_inventory_is_retracted() {
    let facts = Facts {
        listed: false,
        ..Facts::current()
    }
    .build();
    assert_eq!(
        classify(&occurrence(1), Some(&facts)),
        CandidateState::Retracted
    );
    // A successor still wins, as it does over an invalidated registry row.
    let facts = Facts {
        listed: false,
        superseded_by: Some("decision-object-2"),
        ..Facts::current()
    }
    .build();
    assert_eq!(
        classify(&occurrence(1), Some(&facts)),
        CandidateState::Superseded
    );
    // The inventory entry must match the row's class and representation, not only its id.
    let mut facts = Facts::current().build();
    facts.occurrences[0].class = OccurrenceClass::PromotedMemory;
    facts.occurrences[0].representation = "summary";
    assert_eq!(
        classify(&occurrence(1), Some(&facts)),
        CandidateState::Retracted
    );
}

#[test]
fn a_candidate_from_another_batch_reads_no_facts() {
    let mut other = Facts::current().build();
    other.object.object_id = "decision-object-2".to_string();
    let dir = tempfile::tempdir().unwrap();
    let kernel = kernel::KernelStore::open(dir.path()).unwrap();
    let batch = ClaimCandidateBatch {
        known_as_of: 7,
        incarnation: kernel.capture_commit_read_target().unwrap().incarnation,
        claims: vec![other],
        candidates: Vec::new(),
    };
    let mut foreign = ClaimCandidate {
        row: occurrence(1),
        state: CandidateState::Current,
        claim: Some(0),
    };
    // In range, but the facts at that index belong to another object.
    assert_eq!(batch.claim(&foreign), None);
    foreign.claim = Some(1);
    assert_eq!(
        batch.claim(&foreign),
        None,
        "an out-of-range index reads no facts"
    );
    let own = ClaimCandidate {
        row: ClaimCandidateRow {
            object_id: "decision-object-2".to_string(),
            ..occurrence(1)
        },
        state: CandidateState::Current,
        claim: Some(0),
    };
    assert_eq!(batch.claim(&own), Some(&batch.claims[0]));
}

#[test]
fn causal_class_changes_no_state() {
    for served_visibility in [SurfaceVisibility::Labeled, SurfaceVisibility::Hidden] {
        for disposition in [
            Disposition::Active,
            Disposition::Stale,
            Disposition::Disputed,
            Disposition::Superseded,
            Disposition::Rejected,
            Disposition::Contradicted,
            Disposition::Quarantined,
        ] {
            for revision in [1, 2] {
                let states: Vec<CandidateState> = [
                    CausalClass::Unknown(UnknownReason::NoRecord),
                    CausalClass::Unknown(UnknownReason::EvidenceUnavailable),
                    CausalClass::DirectObservation {
                        acquisition_evidence_id: "evidence".to_string(),
                        artifact_digest: "0".repeat(64),
                    },
                    CausalClass::DerivedReinjection {
                        parents: Vec::new(),
                    },
                ]
                .into_iter()
                .map(|causality| {
                    let facts = Facts {
                        revision,
                        disposition: Some(disposition),
                        served: served(served_visibility),
                        causality,
                        ..Facts::current()
                    }
                    .build();
                    classify(&occurrence(1), Some(&facts))
                })
                .collect();
                assert!(
                    states.windows(2).all(|pair| pair[0] == pair[1]),
                    "{served_visibility:?} {disposition:?} {revision}: {states:?}"
                );
            }
        }
    }
}

mod live_rows {
    use std::num::NonZeroUsize;

    use kernel::source_identity::{Occurrence, OccurrenceClass, encode};
    use retrieval::claims::{
        ClaimCandidateBounds, ClaimCandidateError, classify_live_claims, live_claim_candidates,
    };
    use retrieval::{ProjectionError, ProjectionIdentity, install_identity};
    use rusqlite::params;
    use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

    const KERNEL: &str = "kernel-incarnation-a";

    fn identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
        ProjectionIdentity {
            schema_version: retrieval::SCHEMA_VERSION,
            kernel_incarnation_id: kernel_incarnation_id.to_string(),
            projection_policy_version: "policy".to_string(),
            identity_contract_version: "search-projection-identity-v3".to_string(),
            limit_manifest_protocol_version: "limits.v1".to_string(),
            embedding_model: "model".to_string(),
            tokenizer_fingerprint: "fingerprint".to_string(),
            analysis_identity: retrieval::lexical::AnalysisIdentity::current()
                .as_str()
                .to_string(),
            vector_dimension: 8,
            generation_epoch: 1,
        }
    }

    fn open(dir: &std::path::Path) -> SqliteStore {
        open_sqlite(
            &StorageDescriptor {
                module_id: "eidnara-test".to_string(),
                storage_namespace: "search-projection".to_string(),
                isolation: Isolation::Module,
                backend: StorageBackend::Sqlite {
                    path: dir
                        .join("search")
                        .join("search.sqlite")
                        .to_string_lossy()
                        .into_owned(),
                },
            },
            retrieval::BASELINE,
        )
        .unwrap()
    }

    /// Row `n` as the extractor would encode it: even rows are canonical claims,
    /// odd rows promoted memory, all for object `obj-n` at revision 1.
    fn encoded(
        n: usize,
    ) -> (
        OccurrenceClass,
        &'static str,
        String,
        kernel::source_identity::EncodedOccurrence,
    ) {
        let (class, field, representation) = if n.is_multiple_of(2) {
            (
                OccurrenceClass::CanonicalClaims,
                "object_id",
                "decision_summary",
            )
        } else {
            (
                OccurrenceClass::PromotedMemory,
                "decision_object_id",
                "summary",
            )
        };
        let object_id = format!("obj-{n:08}");
        let encoded = encode(
            &Occurrence {
                class: class.code(),
                identity: &[(field, object_id.as_str())],
                revision: "1",
                representation,
                span: None,
            },
            "",
        )
        .unwrap();
        (class, representation, object_id, encoded)
    }

    fn occ(n: usize) -> String {
        encoded(n).3.occurrence_id
    }

    fn seed(store: &SqliteStore, count: usize) {
        store
            .with_conn_unfenced(|conn| {
                conn.execute_batch(
                    "INSERT INTO payloads(payload_id,bytes,byte_length,created_at) VALUES ('p',x'00',1,0);
                     INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                         payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                         source_artifact_digest,created_commit_seq,persisted_at)
                     VALUES ('msg',x'01','m','messages',1,'text','p','d','normal','srcdesc:m','e','digest',1,0);",
                )?;
                for n in 1..=count {
                    let (class, representation, object_id, encoded) = encoded(n);
                    conn.execute(
                        "INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                             payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                             source_artifact_digest,created_commit_seq,persisted_at)
                         VALUES (?1,?2,?3,?4,1,?5,'p','d','normal','srcdesc:s','e','digest',1,0)",
                        params![
                            encoded.occurrence_id,
                            encoded.tuple,
                            encoded.lineage_id,
                            class.code(),
                            representation
                        ],
                    )?;
                    conn.execute(
                        "INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                             extraction_version,created_commit_seq)
                         VALUES ('id','canonical_object',CAST(?1 AS BLOB),?2,?1,1,1)",
                        params![object_id, encoded.occurrence_id],
                    )?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn bound(max: usize) -> NonZeroUsize {
        NonZeroUsize::new(max).unwrap()
    }

    #[test]
    fn rows_are_bounded_ordered_and_keyed_to_the_decision_object() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 4);
        let rows = store
            .with_conn(|conn| Ok(live_claim_candidates(conn, bound(4))))
            .unwrap()
            .unwrap();
        // Canonical claims sort before promoted memory, then by occurrence id.
        let mut claims = [occ(2), occ(4)];
        claims.sort();
        let mut memory = [occ(1), occ(3)];
        memory.sort();
        let ids: Vec<&str> = rows.iter().map(|row| row.occurrence_id.as_str()).collect();
        assert_eq!(ids, [&claims[0], &claims[1], &memory[0], &memory[1]]);
        assert!((1..=4).all(|n| {
            rows.iter()
                .any(|row| row.occurrence_id == occ(n) && row.object_id == format!("obj-{n:08}"))
        }));
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(3))))
                .unwrap(),
            Err(ProjectionError::TooManyRecords { count: 4 })
        ));
    }

    #[test]
    fn a_claim_row_without_its_association_or_with_another_extractor_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 2);
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE exact_associations SET extraction_version=7 WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::ExtractionVersionMismatch {
                stored: 7,
                expected: 1
            })
        ));
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "DELETE FROM exact_associations WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
    }

    #[test]
    fn an_association_whose_target_disagrees_with_its_key_or_row_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 2);
        // The key and the row still identify obj-1; the target names obj-2.
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE exact_associations SET target_id='obj-00000002' WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
        // Key and target agree on obj-2, but the row's tuple identifies obj-1.
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE exact_associations SET key=CAST('obj-00000002' AS BLOB)
                     WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
    }

    /// `occurrence_identity_matches` checks the tuple's identity prefix only; the
    /// stored id and the revision, representation, and span columns are checked
    /// against the tuple with `identity_digest` and `derived_lineage_id`.
    #[test]
    fn a_row_whose_columns_disagree_with_its_tuple_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 2);
        // The tuple encodes revision 1; the column now says 2.
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE occurrences SET revision=2 WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
        // Row 2 keeps a consistent tuple and columns but carries row 1's id.
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE occurrences SET revision=1, tuple=(SELECT tuple FROM occurrences WHERE occurrence_id=?2),
                         lineage_id=(SELECT lineage_id FROM occurrences WHERE occurrence_id=?2),
                         class=(SELECT class FROM occurrences WHERE occurrence_id=?2),
                         representation=(SELECT representation FROM occurrences WHERE occurrence_id=?2)
                     WHERE occurrence_id=?1",
                    [occ(1), occ(2)],
                )?;
                conn.execute(
                    "UPDATE exact_associations SET key=CAST('obj-00000002' AS BLOB), target_id='obj-00000002'
                     WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
    }

    #[test]
    fn an_association_created_in_another_commit_than_its_row_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 2);
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "UPDATE exact_associations SET created_commit_seq=2 WHERE occurrence_id=?1",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
    }

    #[test]
    fn a_second_canonical_object_association_on_one_row_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 2);
        store
            .with_conn_fenced(|conn| {
                conn.execute(
                    "INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                         extraction_version,created_commit_seq)
                     VALUES ('id','canonical_object',CAST('obj-00000002' AS BLOB),?1,
                         'obj-00000002',1,1)",
                    [occ(1)],
                )
            })
            .unwrap();
        assert!(matches!(
            store
                .with_conn(|conn| Ok(live_claim_candidates(conn, bound(8))))
                .unwrap(),
            Err(ProjectionError::CorruptRow)
        ));
    }

    #[test]
    fn a_projection_from_another_kernel_incarnation_is_refused_before_any_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 2);
        let kernel = kernel::KernelStore::open(dir.path().join("kernel")).unwrap();
        let bounds = ClaimCandidateBounds {
            max_rows: bound(1),
            facts: kernel::ClaimFactBounds {
                max_claims: bound(1),
                max_causal_payload_bytes: std::num::NonZeroU64::new(1 << 16).unwrap(),
            },
        };
        // No identity: refused before the row bound, which two rows would also trip.
        assert!(matches!(
            store
                .with_conn(|conn| Ok(classify_live_claims(conn, &kernel, KERNEL, bounds)))
                .unwrap(),
            Err(ClaimCandidateError::NoIdentity)
        ));
        store
            .with_conn_fenced(|conn| Ok(install_identity(conn, &identity(KERNEL), 1)))
            .unwrap()
            .unwrap();
        let refused = store
            .with_conn(|conn| {
                Ok(classify_live_claims(
                    conn,
                    &kernel,
                    "kernel-incarnation-b",
                    bounds,
                ))
            })
            .unwrap();
        match refused {
            Err(ClaimCandidateError::ForeignKernel {
                kernel_incarnation_id,
            }) => assert_eq!(kernel_incarnation_id, KERNEL),
            other => panic!("expected ForeignKernel, got {other:?}"),
        }
        assert!(matches!(
            store
                .with_conn(|conn| Ok(classify_live_claims(conn, &kernel, KERNEL, bounds)))
                .unwrap(),
            Err(ClaimCandidateError::Projection(
                ProjectionError::TooManyRecords { count: 2 }
            ))
        ));
    }

    #[test]
    fn distinct_objects_past_the_facts_bound_are_refused_before_the_kernel_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 3);
        store
            .with_conn_fenced(|conn| Ok(install_identity(conn, &identity(KERNEL), 1)))
            .unwrap()
            .unwrap();
        let kernel = kernel::KernelStore::open(dir.path().join("kernel")).unwrap();
        // Renaming commit_log makes tip reads fail; TooManyClaims must win over Io.
        let raw = rusqlite::Connection::open(dir.path().join("kernel/kernel.sqlite")).unwrap();
        raw.execute_batch("ALTER TABLE commit_log RENAME TO unavailable_commit_log")
            .unwrap();
        assert_eq!(kernel.tip(), Err(kernel::KernelError::Io));
        let bounds = ClaimCandidateBounds {
            max_rows: bound(8),
            facts: kernel::ClaimFactBounds {
                max_claims: bound(2),
                max_causal_payload_bytes: std::num::NonZeroU64::new(1 << 16).unwrap(),
            },
        };
        let refused = store
            .with_conn(|conn| Ok(classify_live_claims(conn, &kernel, KERNEL, bounds)))
            .unwrap();
        assert!(matches!(
            refused,
            Err(retrieval::claims::ClaimCandidateError::Facts(
                kernel::ClaimFactsError::TooManyClaims
            ))
        ));
        let at_bound = ClaimCandidateBounds {
            facts: kernel::ClaimFactBounds {
                max_claims: bound(3),
                ..bounds.facts
            },
            ..bounds
        };
        assert!(matches!(
            store
                .with_conn(|conn| Ok(classify_live_claims(conn, &kernel, KERNEL, at_bound)))
                .unwrap(),
            Err(retrieval::claims::ClaimCandidateError::Facts(
                kernel::ClaimFactsError::Kernel(kernel::KernelError::Io)
            ))
        ));
    }
}
