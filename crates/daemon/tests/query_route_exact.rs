//! The certified exact lane on the `retrieval.query` route: a whole-request proof that passes final use is answered directly, and every other resolved request continues through hybrid retrieval with the rows the resolver retained.

mod support;

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::time::Duration;

use daemon::query_route::{
    ExactAdmission, ExactCertification, ExactRoute, HybridCause, LaneStatus, Phase, QueryFailure,
    QueryOutcome, Terminal, Uncertified,
};
use retrieval::exact::{CertificateRefusal, Completion, Disqualification, ProofInvalidation};
use retrieval::fusion::Lane;
use serde_json::Value;
use support::query_route::{
    ALL_PHASES, Fixture, QUERY, entry_ids, fixed_epoch, grant, limits, request_budget,
};
use tokio_util::sync::CancellationToken;

/// The phases a direct answer passes: no lexical, dense, admission, fusion, or revalidation phase.
const DIRECT_PHASES: [Phase; 5] = [
    Phase::Probes,
    Phase::Exact,
    Phase::ClaimValidation,
    Phase::Materialization,
    Phase::Response,
];

/// The source tuple of each occurrence, read from the projection's occurrence rows rather than from the route.
fn source_tuples(fixture: &Fixture, ids: &[String]) -> BTreeSet<(String, String, i64, String)> {
    fixture
        .projection
        .read(|conn| {
            let mut tuples = BTreeSet::new();
            for id in ids {
                tuples.insert(conn.query_row(
                    "SELECT occurrence_id,source_object_id,revision,representation
                     FROM occurrences WHERE occurrence_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?);
            }
            Ok(tuples)
        })
        .unwrap()
}

/// The occurrences a claim decision projected, read from the projection's occurrence rows.
fn projected(fixture: &Fixture, object: &str) -> BTreeSet<String> {
    fixture
        .projection
        .read(|conn| {
            let mut statement = conn.prepare(
                "SELECT o.occurrence_id FROM occurrences o
                 JOIN exact_associations a ON a.occurrence_id=o.occurrence_id
                 WHERE a.family='id' AND a.key=?1",
            )?;
            let rows = statement
                .query_map([object.as_bytes()], |row| row.get::<_, String>(0))?
                .collect::<Result<BTreeSet<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
}

fn canonical(body: &Value) -> BTreeSet<(String, i64)> {
    body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["canonical"]["decision_object_id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                entry["canonical"]["source_revision"].as_i64().unwrap(),
            )
        })
        .collect()
}

fn retained(outcome: &QueryOutcome) -> BTreeSet<String> {
    outcome
        .exact
        .resolutions
        .iter()
        .flat_map(|resolution| resolution.retained.iter())
        .map(|row| row.occurrence_id.clone())
        .collect()
}

fn exact_fused(outcome: &QueryOutcome) -> BTreeSet<String> {
    outcome
        .fused
        .entries()
        .iter()
        .filter(|entry| entry.lane(Lane::Exact).is_some())
        .map(|entry| entry.occurrence().to_string())
        .collect()
}

/// AC1: a healthy explicit proof skips lexical, dense, admission, fusion, and revalidation, and still validates every served claim against its canonical decision. A forced-hybrid execution of the same request passes the fusion phase, so an implementation that always routes through hybrid retrieval fails the phase assertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_healthy_explicit_proof_is_answered_directly_and_still_validated() {
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let admission = grant(CancellationToken::new());
    let (_token, budget) = request_budget(10_000);
    let mut phases = Vec::new();
    let outcome = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission),
            |phase| phases.push(phase),
        )
        .unwrap();
    assert_eq!(outcome.exact.route, ExactRoute::Direct);
    assert_eq!(phases, DIRECT_PHASES.to_vec());
    assert_eq!(outcome.exact.admission, ExactAdmission::Resolved);
    let [resolution] = outcome.exact.resolutions.as_slice() else {
        panic!("{:?}", outcome.exact.resolutions);
    };
    assert_eq!(resolution.completion, Completion::Complete);
    assert_eq!(resolution.disqualified, None);
    assert!(resolution.proof.is_some());
    assert_eq!(
        outcome.statuses,
        [
            LaneStatus::Complete,
            LaneStatus::Undeclared,
            LaneStatus::Undeclared
        ]
    );
    assert_eq!(outcome.revalidation, None);
    let ids = entry_ids(&outcome.body);
    assert!(!ids.is_empty(), "{}", outcome.body);
    assert_eq!(
        ids.iter().cloned().collect::<BTreeSet<_>>(),
        projected(&fixture, "rule")
    );
    assert_eq!(
        canonical(&outcome.body),
        BTreeSet::from([("rule".into(), 1)])
    );
    for (index, entry) in outcome.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(entry["position"], index + 1);
        assert_eq!(entry["lanes"]["exact"]["position"], index + 1);
        assert!(entry["lanes"].get("lexical").is_none());
    }

    let mut forced_phases = Vec::new();
    let forced = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission)
                .forcing_hybrid_for_test(),
            |phase| forced_phases.push(phase),
        )
        .unwrap();
    assert_eq!(forced.exact.route, ExactRoute::Hybrid(HybridCause::Forced));
    assert!(forced_phases.contains(&Phase::Fusion), "{forced_phases:?}");
    assert_ne!(
        forced_phases, phases,
        "a direct answer is observably unfused"
    );
    fixture.daemon.shutdown().await;
}

/// AC2: separate direct and forced-hybrid executions at one canonical snapshot serve the same canonical decision and revision and the same occurrences with the same source, revision, and representation, each matching the fixture's authored decision and the projection's own occurrence rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_and_forced_hybrid_executions_agree_on_the_target_and_provenance() {
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let admission = grant(CancellationToken::new());
    let (_token, budget) = request_budget(10_000);
    let run = |forced: bool| {
        let certification =
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission);
        fixture
            .run_certified(
                &limits(),
                budget.shared(),
                "id:rule",
                if forced {
                    certification.forcing_hybrid_for_test()
                } else {
                    certification
                },
                |_| {},
            )
            .unwrap()
    };
    let direct = run(false);
    let hybrid = run(true);
    assert_eq!(direct.exact.route, ExactRoute::Direct);
    assert_eq!(hybrid.exact.route, ExactRoute::Hybrid(HybridCause::Forced));
    let expected = projected(&fixture, "rule");
    for outcome in [&direct, &hybrid] {
        let ids = entry_ids(&outcome.body);
        assert_eq!(ids.iter().cloned().collect::<BTreeSet<_>>(), expected);
        assert_eq!(
            canonical(&outcome.body),
            BTreeSet::from([("rule".into(), 1)])
        );
        let tuples = source_tuples(&fixture, &ids);
        // The fixture authors one decision revision whose summary, rationale, and decision summary project as three representations.
        assert_eq!(
            tuples
                .iter()
                .map(|(_, _, revision, representation)| (*revision, representation.as_str()))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(1, "decision_summary"), (1, "rationale"), (1, "summary")]),
            "{tuples:?}"
        );
    }
    assert_eq!(
        source_tuples(&fixture, &entry_ids(&direct.body)),
        source_tuples(&fixture, &entry_ids(&hybrid.body))
    );
    assert!(hybrid.revalidation.is_some());
    fixture.daemon.shutdown().await;
}

/// AC2: every resolved request without a whole-request proof continues through hybrid retrieval, and every row the resolver retained reaches fusion with an exact contribution before any downstream bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_unproven_request_hands_its_retained_rows_to_hybrid_retrieval() {
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let admission = grant(CancellationToken::new());
    let (_token, budget) = request_budget(10_000);
    let certified = || ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission);

    let mixed = fixture
        .run_certified(&limits(), budget.shared(), QUERY, certified(), |_| {})
        .unwrap();
    assert_eq!(mixed.exact.route, ExactRoute::Hybrid(HybridCause::Mention));
    assert!(mixed.exact.resolutions[0].proof.is_none());
    assert_eq!(retained(&mixed), projected(&fixture, "rule"));
    assert_eq!(exact_fused(&mixed), retained(&mixed));
    assert!(
        mixed
            .fused
            .entries()
            .iter()
            .any(|entry| entry.lane(Lane::Lexical).is_some()),
        "the prose still feeds the lexical lane"
    );

    let multiple = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule id:other",
            certified(),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        multiple.exact.route,
        ExactRoute::Hybrid(HybridCause::Mention)
    );
    assert_eq!(multiple.exact.resolutions.len(), 2);
    assert_eq!(exact_fused(&multiple), retained(&multiple));

    let absent = fixture
        .run_certified(&limits(), budget.shared(), "id:absent", certified(), |_| {})
        .unwrap();
    assert_eq!(
        absent.exact.route,
        ExactRoute::Hybrid(HybridCause::Unproven)
    );
    assert_eq!(absent.exact.resolutions[0].completion, Completion::NoMatch);
    assert!(entry_ids(&absent.body).is_empty());

    let rows = projected(&fixture, "rule").len();
    assert!(rows >= 2, "the incomplete case needs a second row");
    let mut bounded = limits();
    bounded.exact_page_rows = NonZeroUsize::new(1).unwrap();
    bounded.exact_pages = NonZeroUsize::new(1).unwrap();
    let incomplete = fixture
        .run_certified(&bounded, budget.shared(), "id:rule", certified(), |_| {})
        .unwrap();
    assert_eq!(
        incomplete.exact.route,
        ExactRoute::Hybrid(HybridCause::Unproven)
    );
    assert!(matches!(
        incomplete.exact.resolutions[0].completion,
        Completion::Incomplete(_)
    ));
    assert_eq!(incomplete.statuses[0], LaneStatus::incomplete("page_bound"));
    assert_eq!(retained(&incomplete).len(), 1);
    assert_eq!(exact_fused(&incomplete), retained(&incomplete));
    assert_eq!(incomplete.body["degraded"], true);
    fixture.daemon.shutdown().await;
}

/// AC2: a moved kernel tip and a non-OK verdict disqualify the attempt from a direct answer; every row the resolver retains reaches fusion, and only eligible rows are served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_authority_and_non_ok_rows_never_answer_directly() {
    let fixture = Fixture::build().await;
    let admission = grant(CancellationToken::new());
    let (_token, budget) = request_budget(10_000);

    let certificate = fixture.certificate();
    fixture.move_kernel_snapshot();
    let lagging = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        lagging.exact.route,
        ExactRoute::Hybrid(HybridCause::Unproven)
    );
    assert!(matches!(
        lagging.exact.resolutions[0].disqualified,
        Some(Disqualification::ProjectionLag { .. })
    ));
    assert_eq!(retained(&lagging), projected(&fixture, "rule"));
    assert_eq!(exact_fused(&lagging), retained(&lagging));
    assert_eq!(
        canonical(&lagging.body),
        BTreeSet::from([("rule".into(), 1)])
    );

    fixture.retire("other").await;
    let retired = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:other",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        retired.exact.route,
        ExactRoute::Hybrid(HybridCause::Unproven)
    );
    let resolution = &retired.exact.resolutions[0];
    assert!(resolution.disqualified.is_some());
    assert!(resolution.proof.is_none());
    let observations = &resolution.observations;
    assert!(
        observations.retracted + observations.superseded + observations.stale + observations.hidden
            > 0,
        "the retired decision's rows are judged non-OK: {observations:?}"
    );
    assert_eq!(observations.eligible, retained(&retired).len());
    assert_eq!(exact_fused(&retired), retained(&retired));
    assert!(
        entry_ids(&retired.body).is_empty(),
        "no retired row is served: {}",
        retired.body
    );
    fixture.daemon.shutdown().await;
}

/// AC4: a certificate the projection refuses, a missing certificate, an admission withdrawn before resolution or before final use, an inventory epoch that moves before final use, and a canonical change before final use each form no direct answer, and the request is still served from the same rows; a healthy admitted control reaches the direct answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_or_withdrawn_authority_falls_back_and_a_healthy_control_answers_directly() {
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let (_token, budget) = request_budget(10_000);
    let healthy = grant(CancellationToken::new());
    let baseline = fixture
        .run(&limits(), budget.shared(), "id:rule", |_| {})
        .unwrap();
    assert_eq!(
        baseline.exact.route,
        ExactRoute::Uncertified(Uncertified::NotSought)
    );
    let served = entry_ids(&baseline.body);
    assert!(!served.is_empty());

    let control = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &healthy),
            |_| {},
        )
        .unwrap();
    assert_eq!(control.exact.route, ExactRoute::Direct);
    assert_eq!(entry_ids(&control.body), served);

    let other_epoch = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(
                certificate.clone(),
                &|| "inventory-other".to_string(),
                &healthy,
            ),
            |_| {},
        )
        .unwrap();
    assert!(matches!(
        other_epoch.exact.route,
        ExactRoute::Uncertified(Uncertified::Refused(
            CertificateRefusal::InventoryEpochMismatch { .. }
        ))
    ));
    assert_eq!(entry_ids(&other_epoch.body), served);

    let mut stale = certificate.clone();
    stale.complete_through_commit_seq -= 1;
    let mismatched = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(stale.clone(), &fixed_epoch, &healthy),
            |_| {},
        )
        .unwrap();
    assert!(matches!(
        mismatched.exact.route,
        ExactRoute::Uncertified(Uncertified::Refused(_))
    ));
    assert_eq!(entry_ids(&mismatched.body), served);

    // A withdrawn admission admits no exact work: before resolution the lane reads pages, and at final use the resolution is discarded for page reads.
    let withdrawn = CancellationToken::new();
    withdrawn.cancel();
    let revoked_grant = grant(withdrawn);
    let unresolved = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &revoked_grant),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        unresolved.exact.route,
        ExactRoute::Uncertified(Uncertified::Withdrawn)
    );
    assert!(unresolved.exact.resolutions.is_empty());
    assert_ne!(unresolved.exact.admission, ExactAdmission::Resolved);
    assert_eq!(entry_ids(&unresolved.body), served);
    let withdrawing = CancellationToken::new();
    let late_grant = grant(withdrawing.clone());
    let revoke = || withdrawing.cancel();
    let revoked = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &late_grant)
                .before_final_use_for_test(&revoke),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        revoked.exact.route,
        ExactRoute::Uncertified(Uncertified::Withdrawn)
    );
    assert_ne!(revoked.exact.admission, ExactAdmission::Resolved);
    assert_eq!(entry_ids(&revoked.body), served);

    // The inventory epoch is read again at final use, so a lease change after resolution invalidates the proof.
    let moved = std::cell::Cell::new(false);
    let moving_epoch = || {
        if moved.get() {
            "inventory-moved".to_string()
        } else {
            fixed_epoch()
        }
    };
    let move_epoch = || moved.set(true);
    let epoch_changed = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &moving_epoch, &healthy)
                .before_final_use_for_test(&move_epoch),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        epoch_changed.exact.route,
        ExactRoute::Hybrid(HybridCause::FinalUse(
            ProofInvalidation::InventoryEpochChanged
        ))
    );
    assert_eq!(entry_ids(&epoch_changed.body), served);

    let commit = || fixture.move_kernel_snapshot();
    let invalidated = fixture
        .run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &healthy)
                .before_final_use_for_test(&commit),
            |_| {},
        )
        .unwrap();
    assert!(
        matches!(
            invalidated.exact.route,
            ExactRoute::Hybrid(HybridCause::FinalUse(
                ProofInvalidation::CanonicalChanged { .. }
            ))
        ),
        "{:?}",
        invalidated.exact.route
    );
    assert_eq!(entry_ids(&invalidated.body), served);
    assert!(invalidated.revalidation.is_some());
    fixture.daemon.shutdown().await;
}

/// AC3: one request budget spans resolution, final use, and every later stage. Cancellation or a deadline at each direct phase, and inside the final-use window, ends the request typed and reaches no later phase; a mixed request whose budget ends in the exact phase reaches no lexical phase.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_original_budget_ends_a_direct_or_resolved_request_at_every_stage() {
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let admission = grant(CancellationToken::new());
    for target in DIRECT_PHASES {
        let (token, budget) = request_budget(10_000);
        let mut reached = Vec::new();
        let outcome = fixture.run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission),
            |phase| {
                reached.push(phase);
                if phase == target {
                    token.cancel();
                }
            },
        );
        assert_eq!(
            outcome.err(),
            Some(QueryFailure::Terminal(Terminal::Cancelled)),
            "cancelled at {target:?}"
        );
        assert_eq!(reached.last(), Some(&target), "{reached:?}");
    }
    for target in DIRECT_PHASES {
        let (_token, budget) = request_budget(600);
        let mut reached = Vec::new();
        let outcome = fixture.run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission),
            |phase| {
                reached.push(phase);
                if phase == target {
                    std::thread::sleep(Duration::from_millis(700));
                }
            },
        );
        assert_eq!(
            outcome.err(),
            Some(QueryFailure::Terminal(Terminal::Deadline)),
            "deadline at {target:?}"
        );
        assert_eq!(reached.last(), Some(&target), "{reached:?}");
    }

    let (token, budget) = request_budget(10_000);
    let cancel = || token.cancel();
    let mut reached = Vec::new();
    let outcome = fixture.run_certified(
        &limits(),
        budget.shared(),
        "id:rule",
        ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission)
            .before_final_use_for_test(&cancel),
        |phase| reached.push(phase),
    );
    assert_eq!(
        outcome.err(),
        Some(QueryFailure::Terminal(Terminal::Cancelled)),
        "an ended budget at final use stops the request instead of falling back"
    );
    assert_eq!(reached, vec![Phase::Probes, Phase::Exact]);

    for target in ALL_PHASES {
        let (token, budget) = request_budget(10_000);
        let mut reached = Vec::new();
        let outcome = fixture.run_certified(
            &limits(),
            budget.shared(),
            QUERY,
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission),
            |phase| {
                reached.push(phase);
                if phase == target {
                    token.cancel();
                }
            },
        );
        assert_eq!(
            outcome.err(),
            Some(QueryFailure::Terminal(Terminal::Cancelled)),
            "cancelled at {target:?}"
        );
        assert_eq!(
            reached.last(),
            Some(&target),
            "no lane continues after the global stop: {reached:?}"
        );
    }
    fixture.daemon.shutdown().await;
}

/// AC3: resolution and final use wait on the kernel only until the request's own deadline. With every kernel reader held well past that deadline, a direct request ends `deadline` near the deadline instead of when the hold ends, both when the hold covers resolution and when it starts at final use; a stage running under a renewed or unbounded budget would wait out the hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kernel_waits_inside_resolution_and_final_use_end_at_the_request_deadline() {
    const HOLD: Duration = Duration::from_secs(4);
    const BUDGET_MS: u64 = 800;
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let admission = grant(CancellationToken::new());
    let holders = std::sync::Mutex::new(Vec::new());
    let hold_readers = || {
        let held = std::sync::Arc::new(std::sync::Barrier::new(2));
        let holder = std::sync::Arc::clone(&held);
        let store = std::sync::Arc::clone(&fixture.store);
        holders.lock().unwrap().push(std::thread::spawn(move || {
            store.hold_readers_for_test(&holder, HOLD)
        }));
        held.wait();
    };
    for stage in ["resolution", "final use"] {
        let (_token, budget) = request_budget(BUDGET_MS);
        let started = std::time::Instant::now();
        let mut reached = Vec::new();
        let at_final_use = || {
            if stage == "final use" {
                hold_readers();
            }
        };
        let outcome = fixture.run_certified(
            &limits(),
            budget.shared(),
            "id:rule",
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission)
                .before_final_use_for_test(&at_final_use),
            |phase| {
                reached.push(phase);
                if phase == Phase::Exact && stage == "resolution" {
                    hold_readers();
                }
            },
        );
        let elapsed = started.elapsed();
        assert_eq!(
            outcome.err(),
            Some(QueryFailure::Terminal(Terminal::Deadline)),
            "{stage}"
        );
        assert_eq!(reached, vec![Phase::Probes, Phase::Exact], "{stage}");
        assert!(
            elapsed < HOLD - Duration::from_secs(1),
            "{stage} waited {elapsed:?}, past its {BUDGET_MS} ms deadline"
        );
        let held: Vec<_> = holders.lock().unwrap().drain(..).collect();
        assert_eq!(held.len(), 1, "{stage} held the readers once");
        for holder in held {
            holder.join().unwrap();
        }
    }
    fixture.daemon.shutdown().await;
}

/// AC2: downstream bounds apply to the direct and hybrid answers alike. A union bound below the retained rows refuses both; a one-row result bound serves the same first entry from both, marks both truncated, and every omitted retained row stays in the ranking the result bound cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downstream_bounds_apply_to_direct_and_hybrid_answers_alike() {
    let fixture = Fixture::build().await;
    let certificate = fixture.certificate();
    let admission = grant(CancellationToken::new());
    let (_token, budget) = request_budget(10_000);
    let run = |limits: &daemon::query_route::QueryRouteLimits, forced: bool| {
        let certification =
            ExactCertification::for_test(certificate.clone(), &fixed_epoch, &admission);
        fixture.run_certified(
            limits,
            budget.shared(),
            "id:rule",
            if forced {
                certification.forcing_hybrid_for_test()
            } else {
                certification
            },
            |_| {},
        )
    };
    let rows = projected(&fixture, "rule").len();
    assert!(rows >= 2);
    let mut union = limits();
    union.fused_union = NonZeroUsize::new(rows - 1).unwrap();
    for forced in [false, true] {
        assert_eq!(
            run(&union, forced).err(),
            Some(QueryFailure::Unavailable("fused_union")),
            "forced hybrid: {forced}"
        );
    }
    let mut one = limits();
    one.result_rows = NonZeroUsize::new(1).unwrap();
    let direct = run(&one, false).unwrap();
    let hybrid = run(&one, true).unwrap();
    assert_eq!(direct.exact.route, ExactRoute::Direct);
    for outcome in [&direct, &hybrid] {
        assert_eq!(outcome.body["truncated"], true);
        let served = entry_ids(&outcome.body);
        assert_eq!(served.len(), 1);
        let ranked: BTreeSet<String> = outcome
            .fused
            .entries()
            .iter()
            .map(|entry| entry.occurrence().to_string())
            .collect();
        for omitted in retained(outcome).difference(&served.iter().cloned().collect()) {
            assert!(
                ranked.contains(omitted) || outcome.fused.entries().len() > served.len(),
                "an omitted row is cut by the result bound: {omitted}"
            );
        }
    }
    assert_eq!(entry_ids(&direct.body), entry_ids(&hybrid.body));
    fixture.daemon.shutdown().await;
}
