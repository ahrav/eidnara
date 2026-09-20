//! The evaluator's eligibility spec against the kernel. The digest test pins
//! the value (the file equals the in-code table and lists the predicates in
//! the order copied from `judge` by hand); the differential pins the
//! behavior against the store, and its kernel assertion runs before its
//! reducer assertion so the kernel breaks first.

#![cfg(feature = "test-support")]

#[path = "support/eligibility_fixture.rs"]
mod eligibility_fixture;

use std::collections::BTreeSet;
use std::path::Path;

use eligibility_fixture::{PROJECT_A, candidate, fixture, with_artifact};
use eval_core::{
    ArtifactEligibility, Destination, ELIGIBILITY_SPEC_DIGEST, ELIGIBILITY_SPEC_PROTOCOL,
    ExecutionMode, FactTuple, PREDICATES, Predicate, Sensitivity, ServedClass, SpecError,
    StateFacts, Surface, Verdict, Visibility, check_spec, judge_surface, judge_with,
    serialize_spec,
};
use kernel::{
    ArtifactDestination, EgressCandidate, EligibilityCandidate, EligibilityVerdict, KernelStore,
    ProjectScope, SurfaceVisibility,
};
use serde_json::{Value, json};

const FIXTURE: &str = "testdata/eligibility-spec-v1.json";

/// The `judge` short-circuit order, copied from the source by hand.
const JUDGE_ORDER: [&str; 8] = [
    "state_absent",
    "superseded",
    "invalidated",
    "revision_differs",
    "out_of_scope",
    "sensitivity_denies_destination",
    "unserved_or_hidden",
    "artifact_denied",
];

fn fixture_text() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)).unwrap()
}

#[test]
fn the_kernel_fixture_and_the_evaluator_table_pin_one_digest_in_judge_order() {
    let parsed: Value = serde_json::from_str(&fixture_text()).unwrap();
    assert_eq!(
        context_core::canonical_json::protocol_digest(ELIGIBILITY_SPEC_PROTOCOL, &parsed).unwrap(),
        ELIGIBILITY_SPEC_DIGEST
    );
    check_spec(&parsed).unwrap();
    assert_eq!(parsed, serialize_spec(), "the file is the in-code table");
    let names: Vec<&str> = parsed["predicates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, JUDGE_ORDER);
    assert_eq!(parsed["verdicts"].as_array().unwrap().len(), 7);

    // Whitespace is not drift; a reordered predicate, an added verdict, and a
    // changed fact cell are.
    let spaced: Value = serde_json::from_str(&fixture_text().replace('\n', "\n\n\t")).unwrap();
    check_spec(&spaced).unwrap();
    let mutations: [fn(&mut Value); 3] = [
        |f| f["predicates"].as_array_mut().unwrap().swap(5, 6),
        |f| {
            f["verdicts"]
                .as_array_mut()
                .unwrap()
                .push(json!("quarantined"))
        },
        |f| f["vectors"][0]["facts"]["state"]["in_scope"] = json!(false),
    ];
    for mutate in mutations {
        let mut drifted = parsed.clone();
        mutate(&mut drifted);
        assert!(matches!(
            check_spec(&drifted),
            Err(SpecError::SpecDrift { .. })
        ));
    }
}

const fn served(
    sensitivity: Sensitivity,
    explicit: Visibility,
    auto: Visibility,
) -> Option<ServedClass> {
    Some(ServedClass {
        sensitivity,
        visibility: explicit,
        auto_inject: auto,
        auto_search: auto,
    })
}

const LABELED: Option<ServedClass> =
    served(Sensitivity::Normal, Visibility::Labeled, Visibility::Hidden);
const AUTOMATIC: Option<ServedClass> = served(
    Sensitivity::Normal,
    Visibility::Visible,
    Visibility::Visible,
);
const BARRED: Option<ServedClass> =
    served(Sensitivity::Normal, Visibility::Hidden, Visibility::Hidden);
const SECRET: Option<ServedClass> =
    served(Sensitivity::Secret, Visibility::Hidden, Visibility::Hidden);
const SENSITIVE: Option<ServedClass> = served(
    Sensitivity::Sensitive,
    Visibility::Labeled,
    Visibility::Hidden,
);

const OK_STATE: StateFacts = StateFacts {
    superseded: false,
    invalidated: false,
    revision_matches: true,
    in_scope: true,
    registry_sensitivity: Sensitivity::Normal,
};

/// The candidate names an artifact, allowed locally only (`Sensitive`) or nowhere (`Secret`).
#[derive(Clone, Copy)]
enum Cites {
    Nothing,
    SensitiveArtifact,
    SecretArtifact,
}

/// One hand-authored row: the facts the fixture gives an object and what both
/// implementations must say about it, `[local, remote]` verdicts and
/// `[auto_inject, auto_search, explicit_search]` visibilities as literals.
struct Row {
    candidate: EligibilityCandidate,
    state: Option<StateFacts>,
    served: Option<ServedClass>,
    cites: Cites,
    verdicts: [Verdict; 2],
    visibilities: [Visibility; 3],
}

impl Row {
    fn facts(&self, destination: Destination) -> FactTuple {
        FactTuple {
            state: self.state,
            served: self.served,
            artifact: match (self.cites, destination) {
                (Cites::Nothing, _) => None,
                (Cites::SensitiveArtifact, Destination::Local) => {
                    Some(ArtifactEligibility::Allowed)
                }
                (Cites::SensitiveArtifact | Cites::SecretArtifact, _) => {
                    Some(ArtifactEligibility::Denied)
                }
            },
            destination,
        }
    }

    fn verdict(&self, destination: Destination) -> Verdict {
        self.verdicts[destination as usize]
    }
}

fn live(
    object: &str,
    revision: i64,
    served: Option<ServedClass>,
    visibilities: [Visibility; 3],
) -> Row {
    Row {
        candidate: candidate(object, revision),
        state: Some(OK_STATE),
        served,
        cites: Cites::Nothing,
        verdicts: [Verdict::Ok, Verdict::Ok],
        visibilities,
    }
}

const H3: [Visibility; 3] = [Visibility::Hidden, Visibility::Hidden, Visibility::Hidden];
const L: [Visibility; 3] = [Visibility::Hidden, Visibility::Hidden, Visibility::Labeled];
const V3: [Visibility; 3] = [
    Visibility::Visible,
    Visibility::Visible,
    Visibility::Visible,
];

fn table(sensitive_artifact: &str, secret_artifact: &str) -> Vec<Row> {
    use Sensitivity::{Secret, Sensitive};
    use Verdict::*;
    let both = |verdict| [verdict, verdict];
    let gone = |object: &str, revision: i64, state: StateFacts, verdict: Verdict| Row {
        state: Some(state),
        verdicts: both(verdict),
        ..live(object, revision, None, H3)
    };
    let stateful = |row: Row, state: StateFacts, verdict: Verdict| Row {
        state: Some(state),
        verdicts: both(verdict),
        ..row
    };
    vec![
        live("ok", 1, LABELED, L),
        live("automatic", 1, AUTOMATIC, V3),
        gone(
            "retired",
            1,
            StateFacts {
                invalidated: true,
                ..OK_STATE
            },
            Retracted,
        ),
        gone(
            "replaced",
            1,
            StateFacts {
                superseded: true,
                invalidated: true,
                ..OK_STATE
            },
            Superseded,
        ),
        live("replacement", 2, LABELED, L),
        stateful(
            live("replacement", 1, LABELED, L),
            StateFacts {
                revision_matches: false,
                ..OK_STATE
            },
            Stale,
        ),
        stateful(
            live("stale", 7, LABELED, L),
            StateFacts {
                revision_matches: false,
                ..OK_STATE
            },
            Stale,
        ),
        stateful(
            live("other-project", 1, LABELED, L),
            StateFacts {
                in_scope: false,
                ..OK_STATE
            },
            WrongScope,
        ),
        stateful(
            live("branch-only-scope", 1, LABELED, L),
            StateFacts {
                in_scope: false,
                ..OK_STATE
            },
            WrongScope,
        ),
        stateful(
            live("unscoped", 1, LABELED, L),
            StateFacts {
                in_scope: false,
                ..OK_STATE
            },
            WrongScope,
        ),
        stateful(
            live("secret", 1, SECRET, H3),
            StateFacts {
                registry_sensitivity: Secret,
                ..OK_STATE
            },
            ProviderSensitive,
        ),
        Row {
            state: Some(StateFacts {
                registry_sensitivity: Sensitive,
                ..OK_STATE
            }),
            verdicts: [Ok, ProviderSensitive],
            ..live("sensitive", 1, SENSITIVE, L)
        },
        Row {
            verdicts: both(Hidden),
            ..live("unadmitted", 1, None, H3)
        },
        Row {
            verdicts: both(Hidden),
            ..live("contradicted", 1, BARRED, H3)
        },
        Row {
            candidate: with_artifact(candidate("with-artifact", 1), sensitive_artifact),
            cites: Cites::SensitiveArtifact,
            verdicts: [Ok, ProviderSensitive],
            ..live("with-artifact", 1, LABELED, L)
        },
        Row {
            candidate: with_artifact(candidate("with-artifact", 1), secret_artifact),
            cites: Cites::SecretArtifact,
            verdicts: both(ProviderSensitive),
            ..live("with-artifact", 1, LABELED, L)
        },
        Row {
            state: None,
            verdicts: both(Retracted),
            ..live("never-written", 1, None, H3)
        },
        // Precedence pairs: two faults each, the earlier predicate names the verdict.
        gone(
            "replaced",
            9,
            StateFacts {
                superseded: true,
                invalidated: true,
                revision_matches: false,
                ..OK_STATE
            },
            Superseded,
        ),
        gone(
            "retired",
            9,
            StateFacts {
                invalidated: true,
                revision_matches: false,
                ..OK_STATE
            },
            Retracted,
        ),
        gone(
            "retired-other-project",
            1,
            StateFacts {
                invalidated: true,
                in_scope: false,
                ..OK_STATE
            },
            Retracted,
        ),
        stateful(
            live("stale-other-project", 9, LABELED, L),
            StateFacts {
                revision_matches: false,
                in_scope: false,
                ..OK_STATE
            },
            Stale,
        ),
        stateful(
            live("secret-other-project", 1, SECRET, H3),
            StateFacts {
                in_scope: false,
                registry_sensitivity: Secret,
                ..OK_STATE
            },
            WrongScope,
        ),
        stateful(
            live("secret-unadmitted", 1, None, H3),
            StateFacts {
                registry_sensitivity: Secret,
                ..OK_STATE
            },
            ProviderSensitive,
        ),
        Row {
            candidate: with_artifact(
                candidate("contradicted-with-artifact", 1),
                sensitive_artifact,
            ),
            cites: Cites::SensitiveArtifact,
            verdicts: both(Hidden),
            ..live("contradicted-with-artifact", 1, BARRED, H3)
        },
    ]
}

// A variant added to a kernel enum fails to compile here, so the mirrored
// eval-core enum cannot go stale silently.

fn destination(destination: ArtifactDestination) -> Destination {
    match destination {
        ArtifactDestination::Local => Destination::Local,
        ArtifactDestination::Remote => Destination::Remote,
    }
}

fn surface(surface: kernel::Surface) -> Surface {
    match surface {
        kernel::Surface::AutoInject => Surface::AutoInject,
        kernel::Surface::AutoSearch => Surface::AutoSearch,
        kernel::Surface::ExplicitSearch => Surface::ExplicitSearch,
    }
}

fn verdict(verdict: EligibilityVerdict) -> Verdict {
    match verdict {
        EligibilityVerdict::Ok => Verdict::Ok,
        EligibilityVerdict::Retracted => Verdict::Retracted,
        EligibilityVerdict::Superseded => Verdict::Superseded,
        EligibilityVerdict::Stale => Verdict::Stale,
        EligibilityVerdict::WrongScope => Verdict::WrongScope,
        EligibilityVerdict::Hidden => Verdict::Hidden,
        EligibilityVerdict::ProviderSensitive => Verdict::ProviderSensitive,
    }
}

fn visibility(visibility: SurfaceVisibility) -> Visibility {
    match visibility {
        SurfaceVisibility::Hidden => Visibility::Hidden,
        SurfaceVisibility::Visible => Visibility::Visible,
        SurfaceVisibility::Labeled => Visibility::Labeled,
    }
}

fn sensitivity(sensitivity: kernel::Sensitivity) -> Sensitivity {
    match sensitivity {
        kernel::Sensitivity::Normal => Sensitivity::Normal,
        kernel::Sensitivity::Sensitive => Sensitivity::Sensitive,
        kernel::Sensitivity::Secret => Sensitivity::Secret,
    }
}

/// The shell-side projection of kernel egress facts onto the value tuple the
/// reducer judges. The scope decision is the caller's, made from the state's
/// scope id; an object without state has no scope.
fn fact_tuple(
    candidate: &EligibilityCandidate,
    egress: &EgressCandidate,
    destination: Destination,
    in_scope: bool,
) -> FactTuple {
    FactTuple {
        state: egress.state.as_ref().map(|state| StateFacts {
            superseded: state.object.superseded_by.is_some(),
            invalidated: state.object.invalidated_commit_seq.is_some(),
            revision_matches: state.object.source_revision == candidate.source_revision,
            in_scope: in_scope && state.scope_id.is_some(),
            registry_sensitivity: sensitivity(state.object.sensitivity),
        }),
        served: egress.served.map(|served| ServedClass {
            sensitivity: sensitivity(served.sensitivity),
            visibility: visibility(served.visibility),
            auto_inject: visibility(served.auto_inject),
            auto_search: visibility(served.auto_search),
        }),
        artifact: egress.artifact.map(|artifact| match artifact.eligibility {
            kernel::ArtifactEligibility::Allowed => ArtifactEligibility::Allowed,
            kernel::ArtifactEligibility::Denied(_) => ArtifactEligibility::Denied,
        }),
        destination,
    }
}

/// Enumerate mode: every table row, both destinations, all three surfaces.
#[test]
fn the_reducer_agrees_with_the_kernel_on_the_hand_authored_fact_tuple_table() {
    let store_fixture = fixture();
    let store: &KernelStore = &store_fixture.store;
    let project = ProjectScope::new(PROJECT_A).unwrap();
    let rows = table(
        &store_fixture.sensitive_artifact,
        &store_fixture.secret_artifact,
    );
    let candidates: Vec<EligibilityCandidate> = rows.iter().map(|r| r.candidate.clone()).collect();
    let named: Vec<(String, Option<String>)> = candidates
        .iter()
        .map(|c| (c.object_id.clone(), c.artifact_digest.clone()))
        .collect();
    let mut seen = BTreeSet::new();
    for &kernel_dest in ArtifactDestination::ALL {
        let dest = destination(kernel_dest);
        let (_, egress) = store.egress_candidates(&named, kernel_dest).unwrap();
        assert_eq!(egress.len(), rows.len());
        for (row, egress) in rows.iter().zip(&egress) {
            // The scope decision is the row's own; whether the object has a
            // scope at all comes from the store.
            let in_scope = row.state.is_some_and(|s| s.in_scope);
            assert_eq!(
                fact_tuple(&row.candidate, egress, dest, in_scope),
                row.facts(dest),
                "{} facts {dest:?}",
                row.candidate.object_id
            );
        }
        for &kernel_surf in kernel::Surface::ALL {
            let surf = surface(kernel_surf);
            let batch = store
                .judge_surface_eligibility(&project, kernel_dest, kernel_surf, &candidates)
                .unwrap();
            assert!(batch.snapshot.classification_generation.is_some());
            assert_eq!(batch.verdicts.len(), candidates.len());
            for (row, judged) in rows.iter().zip(&batch.verdicts) {
                let want_verdict = row.verdict(dest);
                let want_visibility = row.visibilities[surf as usize];
                let name = &row.candidate.object_id;
                assert_eq!(
                    verdict(judged.verdict),
                    want_verdict,
                    "kernel {name} {dest:?}"
                );
                assert_eq!(
                    visibility(judged.visibility),
                    want_visibility,
                    "kernel {name} {surf:?}"
                );
                let reduced = judge_surface(&row.facts(dest), surf);
                assert_eq!(reduced.verdict, want_verdict, "reducer {name} {dest:?}");
                assert_eq!(
                    reduced.visibility, want_visibility,
                    "reducer {name} {surf:?}"
                );
                assert_eq!(
                    reduced.permits(),
                    judged.permits(),
                    "{name} {dest:?} {surf:?}"
                );
                seen.insert((want_verdict, surf, reduced.permits()));
            }
        }
    }
    let verdicts: BTreeSet<Verdict> = seen.iter().map(|(v, _, _)| *v).collect();
    assert_eq!(verdicts.len(), 7, "every verdict is reached");
    let surfaces: BTreeSet<Surface> = seen.iter().map(|(_, s, _)| *s).collect();
    assert_eq!(
        surfaces,
        Surface::ALL.into_iter().collect(),
        "the kernel's surface list and the mirror's agree"
    );
    assert_eq!(
        ArtifactDestination::ALL.len(),
        2,
        "the kernel's destination list and the mirror's agree"
    );
    assert!(
        seen.contains(&(Verdict::Ok, Surface::AutoInject, true)),
        "the fold permits automatic rows"
    );
    assert!(
        seen.contains(&(Verdict::Ok, Surface::AutoInject, false)),
        "the fold hides labeled rows"
    );
    assert!(
        rows.iter()
            .any(|r| r.state.is_some_and(|s| s.superseded && s.invalidated))
    );
    // The admission policy gives both automatic surfaces one visibility today;
    // a row that splits them needs a new policy and a new table entry.
    assert!(rows.iter().all(|r| r.visibilities[0] == r.visibilities[1]));
}

/// A wrong precedence disagrees with the table on its own terms, before any
/// digest is compared: every adjacent transposition of the order shows up on
/// some row except the first pair, which no store object can separate.
#[test]
fn every_adjacent_transposition_of_the_order_disagrees_with_the_table() {
    let rows = table(&"ab".repeat(32), &"cd".repeat(32));
    for i in 0..PREDICATES.len() - 1 {
        let mut predicates: Vec<Predicate> = PREDICATES.to_vec();
        predicates.swap(i, i + 1);
        let disagreements = rows
            .iter()
            .flat_map(|row| [Destination::Local, Destination::Remote].map(|d| (row, d)))
            .filter(|(row, d)| judge_with(&predicates, &row.facts(*d)) != row.verdict(*d))
            .count();
        assert_eq!(
            disagreements > 0,
            i != 0,
            "transposition {i}: {disagreements}"
        );
    }
    assert_eq!(
        serde_json::to_value(ExecutionMode::Enumerate).unwrap(),
        json!("enumerate")
    );
}
