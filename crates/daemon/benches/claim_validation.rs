//! Local two-pass claim validation over real canonical decisions and artifacts.
//! Rows model classified projection output; projection building, host transport,
//! selection quality and provider calls are outside the timed boundary.

use daemon::claim_sources::{ClaimMaterializer, MaterializationEnd};
use daemon::kernel_route_fixtures::{
    MEMORY_DOMAIN, admission, decision_spec_in_domain, ensure_domain, intent, project_scope_spec,
    seed_domain,
};
use kernel::applicability::EvalBudget;
use kernel::{
    ArtifactDestination, ClaimFactBounds, CommitPageBounds, EventKind, KernelStore, ProjectScope,
    ProviderEgress, SourceClass, Surface, TaintClass,
};
use retrieval::claims::{
    CandidateState, ClaimCandidate, ClaimCandidateBatch, ClaimCandidateRow, SurfaceValidation,
    UseAccounting, UseVerdict, classify, validate_for_surface,
};
use serde_json::json;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::{Duration, Instant};
#[cfg(not(feature = "bench-internals"))]
use std::{hint::black_box, sync::Barrier};

#[cfg(feature = "bench-internals")]
#[expect(
    dead_code,
    reason = "the shared recorder also serves buffer-provenance tests"
)]
#[path = "../tests/support/alloc_recorder.rs"]
mod alloc_recorder;
#[cfg(feature = "bench-internals")]
#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CASES: [(&str, usize, usize, usize, bool); 6] = [
    ("single", 1, 3, 1, false),
    ("mixed64", 16, 64, 1, false),
    ("duplicates1024", 16, 1024, 1, false),
    ("distinct256", 256, 256, 1, false),
    ("remote64", 16, 64, 1, true),
    ("concurrent64", 16, 64, 4, false),
];

struct Fixture {
    kernel: KernelStore,
    _dir: tempfile::TempDir,
    batch: ClaimCandidateBatch,
    bounds: ClaimFactBounds,
    project: ProjectScope,
    destination: ArtifactDestination,
    expected: Vec<bool>,
}

impl Fixture {
    fn new(objects: usize, rows: usize, remote: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(dir.path()).unwrap();
        seed_domain(&kernel);
        kernel
            .commit(intent("scopes"), |tx| {
                ensure_domain(tx, MEMORY_DOMAIN)?;
                tx.insert_scope(project_scope_spec("own", PROJECT))?;
                tx.insert_scope(project_scope_spec("foreign", &"b".repeat(64)))?;
                Ok(String::new())
            })
            .unwrap();
        ClaimMaterializer::register(&kernel, 1).unwrap();
        let ids: Vec<_> = (0..objects).map(|i| format!("claim-{i:04}")).collect();
        for (i, id) in ids.iter().enumerate() {
            kernel
                .commit(intent(id), |tx| {
                    let scope = if objects > 1 && i % 8 == 1 {
                        "foreign"
                    } else {
                        "own"
                    };
                    let automatic = objects > 1 && i.is_multiple_of(8);
                    let mut spec = decision_spec_in_domain(
                        id,
                        scope,
                        MEMORY_DOMAIN,
                        if automatic {
                            "adr_accepted"
                        } else {
                            "PROJECT_RULES"
                        },
                        &format!("Preserve public contract {id}."),
                    );
                    spec.payload.rationale = format!("Rationale for {id}.");
                    tx.insert_decision(spec)?;
                    tx.record_admission(admission(
                        id,
                        if automatic {
                            EventKind::AcceptedAdr
                        } else {
                            EventKind::Other
                        },
                        None,
                        (SourceClass::ModelInference, TaintClass::AssistantInference),
                    ))?;
                    Ok(String::new())
                })
                .unwrap();
        }
        let report = ClaimMaterializer::new(&kernel, ProviderEgress::LocalOnly)
            .run_episode(
                CommitPageBounds {
                    max_commits: NonZeroUsize::new(64).unwrap(),
                    max_rows: NonZeroUsize::new(4096).unwrap(),
                    max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
                },
                1,
            )
            .unwrap();
        assert!(
            matches!(report.end, MaterializationEnd::ReachedTarget),
            "{report:?}"
        );
        kernel
            .commit(intent("restrict-before-classification"), |tx| {
                for (i, id) in ids.iter().enumerate() {
                    if objects > 1 && i % 8 == 2 {
                        tx.record_admission(admission(
                            id,
                            EventKind::Quarantine,
                            None,
                            (SourceClass::ModelInference, TaintClass::AssistantInference),
                        ))?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
        let target = kernel.capture_commit_read_target().unwrap();
        let tip = target.through_commit;
        let bounds = ClaimFactBounds {
            max_claims: NonZeroUsize::new(objects).unwrap(),
            max_causal_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        };
        let claims = kernel.claim_facts_at(&ids, target, bounds).unwrap().claims;
        assert_eq!(claims.len(), objects);
        assert!(claims.iter().all(|claim| !claim.occurrences.is_empty()));
        let candidates = (0..rows)
            .map(|n| {
                let i = n % objects;
                let claim = &claims[i];
                assert_eq!(claim.object.object_id, ids[i]);
                let occurrence = &claim.occurrences[n / objects % claim.occurrences.len()];
                let row = ClaimCandidateRow {
                    occurrence_id: occurrence.occurrence_id.clone(),
                    class: occurrence.class,
                    representation: occurrence.representation.into(),
                    object_id: claim.object.object_id.clone(),
                    revision: claim.object.source_revision,
                    artifact_digest: occurrence.artifact_digest.clone(),
                };
                ClaimCandidate {
                    state: classify(&row, Some(claim)),
                    row,
                    claim: Some(i),
                }
            })
            .collect();
        kernel
            .commit(intent("restrict-after-classification"), |tx| {
                for (i, id) in ids.iter().enumerate() {
                    if objects > 1 && i % 8 == 3 {
                        tx.record_admission(admission(
                            id,
                            EventKind::Quarantine,
                            None,
                            (SourceClass::ModelInference, TaintClass::AssistantInference),
                        ))?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
        Self {
            kernel,
            _dir: dir,
            batch: ClaimCandidateBatch {
                known_as_of: tip,
                incarnation: target.incarnation,
                claims,
                candidates,
            },
            bounds,
            project: ProjectScope::new(PROJECT).unwrap(),
            destination: if remote {
                ArtifactDestination::Remote
            } else {
                ArtifactDestination::Local
            },
            expected: (0..rows)
                .map(|n| !remote && (objects == 1 || !matches!(n % objects % 8, 1..=3)))
                .collect(),
        }
    }

    fn operation(&self) -> (SurfaceValidation, SurfaceValidation) {
        let budget = EvalBudget::new(
            Some(Instant::now() + Duration::from_secs(30)),
            Default::default(),
        );
        let first = validate_for_surface(
            &self.kernel,
            &self.batch.candidates,
            &self.project,
            self.destination,
            Surface::ExplicitSearch,
            self.bounds,
            self.batch.incarnation,
            &budget,
        )
        .unwrap();
        let survivors: Vec<_> = first
            .candidates
            .iter()
            .filter(|row| matches!(row.verdict, UseVerdict::Permitted(_)))
            .take(16)
            .map(|row| row.candidate.clone())
            .collect();
        let second = validate_for_surface(
            &self.kernel,
            &survivors,
            &self.project,
            self.destination,
            Surface::ExplicitSearch,
            self.bounds,
            self.batch.incarnation,
            &budget,
        )
        .unwrap();
        (first, second)
    }

    fn check(&self, result: &(SurfaceValidation, SurfaceValidation)) {
        let (first, second) = result;
        assert_eq!(first.candidates.len(), self.expected.len());
        assert_eq!(first.accounting.attempted_rows, self.expected.len());
        assert!(first.snapshot.tip >= self.batch.known_as_of);
        assert!(second.snapshot.tip >= first.snapshot.tip);
        assert_eq!(first.incarnation, self.batch.incarnation);
        assert_eq!(second.incarnation, first.incarnation);
        assert_eq!(first.claims.len(), self.batch.claims.len());
        assert!(
            first
                .claims
                .iter()
                .all(|claim| claim.causality.is_unknown())
        );
        assert_eq!(
            second.claims.len(),
            second.accounting.permitted_objects.len()
        );
        let mut accounting = UseAccounting {
            attempted_rows: self.expected.len(),
            ..Default::default()
        };
        for ((row, expected), original) in first
            .candidates
            .iter()
            .zip(&self.expected)
            .zip(&self.batch.candidates)
        {
            assert_eq!(&row.candidate, original);
            assert_eq!(matches!(row.verdict, UseVerdict::Permitted(_)), *expected);
            let id = original.row.object_id.clone();
            if *expected {
                accounting.permitted_objects.insert(id.clone());
            } else {
                accounting.rejected_objects.insert(id.clone());
            }
            accounting.unknown_objects.insert(id);
        }
        assert_eq!(first.accounting, accounting);
        let expected_survivors: Vec<_> = first
            .candidates
            .iter()
            .filter(|row| matches!(row.verdict, UseVerdict::Permitted(_)))
            .take(16)
            .cloned()
            .collect();
        assert_eq!(second.candidates, expected_survivors);
        assert!(second.accounting.rejected_objects.is_empty());
        assert!(
            first
                .candidates
                .iter()
                .any(|row| row.candidate.state == CandidateState::Current)
        );
    }
}

fn affinity() -> String {
    std::fs::read_to_string("/proc/thread-self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:\t"))
        .unwrap()
        .to_string()
}

fn main() {
    let samples: usize =
        std::env::var("EIDNARA_CLAIM_BENCH_SAMPLES").map_or(2, |s| s.parse().unwrap());
    assert!((1..=10000).contains(&samples));
    let filter = std::env::var("EIDNARA_CLAIM_BENCH_CASE").ok();
    assert!(
        filter
            .as_deref()
            .is_none_or(|filter| CASES.iter().any(|case| case.0 == filter))
    );
    for (name, objects, rows, workers, remote) in CASES {
        if filter.as_deref().is_some_and(|filter| filter != name) {
            continue;
        }
        let _ = (workers, samples);
        let fixture = Fixture::new(objects, rows, remote);
        fixture.check(&fixture.operation());
        #[cfg(feature = "bench-internals")]
        {
            let (result, ledger) = alloc_recorder::record_window(|| fixture.operation());
            fixture.check(&result);
            let allocation_events = (!ledger.overflow).then_some(ledger.allocation_events);
            println!(
                "{}",
                json!({"schema":2,"case":name,"mode":"allocations", "allocation_events":allocation_events,
                "requested_bytes":ledger.requested_bytes,"peak_live_bytes":ledger.peak_live_bytes,"affinity":affinity(),
                "workers":1,"outputs_retained_at_close":true,"ledger_overflow":ledger.overflow})
            );
        }
        #[cfg(not(feature = "bench-internals"))]
        {
            let barrier = Barrier::new(workers);
            let results = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..workers)
                    .map(|_| {
                        let fixture = &fixture;
                        let barrier = &barrier;
                        scope.spawn(move || {
                            for _ in 0..8 {
                                black_box(fixture.operation());
                            }
                            let before = affinity();
                            let mut ns = Vec::with_capacity(samples);
                            barrier.wait();
                            for _ in 0..samples {
                                let start = Instant::now();
                                let result = black_box(fixture.operation());
                                drop(result);
                                ns.push(start.elapsed().as_nanos() as u64);
                            }
                            assert_eq!(before, affinity());
                            (before, ns)
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| handle.join().unwrap())
                    .collect::<Vec<_>>()
            });
            fixture.check(&fixture.operation());
            println!(
                "{}",
                json!({"schema":2,"case":name,"mode":"timing","objects":objects,"rows":rows,
                "workers":workers,"samples_per_worker":samples,"results":results,"outcomes":"all_canaries_passed"})
            );
        }
    }
}
