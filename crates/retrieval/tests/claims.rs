//! The candidate state table, exhausted by hand: every combination of the
//! canonical facts `classify` reads, with the expected state written out from
//! the documented precedence rather than derived by the function under test.

use kernel::source_identity::OccurrenceClass;
use kernel::{
    AdmissionFacts, CausalClass, ClaimDecisionFacts, ClaimFacts, Disposition, EventKind, Maturity,
    ObjectRow, Outcome, Sensitivity, ServedFacts, ServedStanding, SourceClass, SurfaceVisibility,
    TaintClass, UnknownReason, VisibilityRow,
};
use retrieval::claims::{CandidateState, ClaimCandidateRow, classify};

fn occurrence(revision: i64) -> ClaimCandidateRow {
    ClaimCandidateRow {
        occurrence_id: "occ".to_string(),
        class: OccurrenceClass::CanonicalClaims,
        representation: "decision_summary".to_string(),
        object_id: "decision-object-1".to_string(),
        revision,
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
    served: ServedStanding,
    causality: CausalClass,
}

impl Facts {
    fn current() -> Self {
        Self {
            invalidated: None,
            superseded_by: None,
            revision: 1,
            disposition: Some(Disposition::Active),
            served: served(SurfaceVisibility::Labeled),
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
            lineage_admission: None,
            served: self.served,
            occurrences: Vec::new(),
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

    use retrieval::ProjectionError;
    use retrieval::claims::{ClaimCandidateBounds, classify_live_claims, live_claim_candidates};
    use storage::{Isolation, SqliteStore, StorageBackend, StorageDescriptor, open_sqlite};

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

    /// `count` live claim rows with associations, plus one message row that must
    /// never appear, written straight into the baseline schema.
    fn seed(store: &SqliteStore, count: usize) {
        store
            .with_conn_unfenced(|conn| {
                conn.execute_batch(&format!(
                    "INSERT INTO payloads(payload_id,bytes,byte_length,created_at) VALUES ('p',x'00',1,0);
                     WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<{count})
                     INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                         payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                         source_artifact_digest,created_commit_seq,persisted_at)
                     SELECT printf('occ-%08d',n),x'00','l',
                         CASE n%2 WHEN 0 THEN 'canonical_claims' ELSE 'promoted_memory' END,
                         1,'decision_summary','p','d','normal','srcdesc:s','e','digest',1,0 FROM ids;
                     WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<{count})
                     INSERT INTO exact_associations(family,namespace,key,occurrence_id,target_id,
                         extraction_version,created_commit_seq)
                     SELECT 'id','canonical_object',CAST(printf('obj-%08d',n) AS BLOB),printf('occ-%08d',n),
                         printf('obj-%08d',n),1,1 FROM ids;
                     INSERT INTO occurrences(occurrence_id,tuple,lineage_id,class,revision,representation,
                         payload_id,domain_id,sensitivity,source_object_id,source_evidence_id,
                         source_artifact_digest,created_commit_seq,persisted_at)
                     VALUES ('msg',x'01','m','messages',1,'text','p','d','normal','srcdesc:m','e','digest',1,0);"
                ))
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
        let ids: Vec<&str> = rows.iter().map(|row| row.occurrence_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "occ-00000002",
                "occ-00000004",
                "occ-00000001",
                "occ-00000003"
            ]
        );
        assert!(
            rows.iter()
                .all(|row| row.object_id == row.occurrence_id.replace("occ", "obj"))
        );
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
                    "UPDATE exact_associations SET extraction_version=7 WHERE occurrence_id='occ-00000001'",
                    [],
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
                    "DELETE FROM exact_associations WHERE occurrence_id='occ-00000001'",
                    [],
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
    fn distinct_objects_past_the_facts_bound_are_refused_before_the_kernel_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path());
        seed(&store, 3);
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
            .with_conn(|conn| Ok(classify_live_claims(conn, &kernel, bounds)))
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
                .with_conn(|conn| Ok(classify_live_claims(conn, &kernel, at_bound)))
                .unwrap(),
            Err(retrieval::claims::ClaimCandidateError::Facts(
                kernel::ClaimFactsError::Kernel(kernel::KernelError::Io)
            ))
        ));
    }
}
