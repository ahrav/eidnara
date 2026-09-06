//! Benchmark suite separates cold-cache, warm-cache, repository-shape, and
//! adversarial inputs so setup costs do not blur kernel measurements.

#[path = "../tests/support/applicability_fixtures.rs"]
mod applicability_fixtures;
#[path = "../tests/support/git_fixtures.rs"]
mod git_fixtures;

use std::hint::black_box;
use std::time::Duration;

use applicability_fixtures::{candidate, checkout, reachable_anchor};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use git_fixtures::{
    FixtureRepo, commit_snapshot, commit_tree, init_repo, materialize, set_head, write_tree,
    write_worktree_file,
};
use kernel::applicability::{
    ApplicabilityCandidate, ApplicabilityEngine, CheckCache, CheckSpec, EvalBudget,
    ObjectApplicabilitySpec, ResolutionLadder, run_cheap_check, snapshot_checkout,
};
use kernel::{
    CanonicalScope, Dimension, GraphOracle, QueryContext, ScopeMatchContext, ScopeTermSpec,
    UnknownGraph, scope_matches, scope_overlaps, scope_subsumes,
};

fn exact(dimension: &str, value: &str) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: dimension.to_string(),
        operator: "exact".to_string(),
        exact_value: Some(value.to_string()),
        ..ScopeTermSpec::default()
    }
}

fn version_range(dimension: &str, value: &str) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: dimension.to_string(),
        operator: "version_range".to_string(),
        version_range: Some(value.to_string()),
        ..ScopeTermSpec::default()
    }
}

fn set_term(dimension: &str, values: &[&str]) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: dimension.to_string(),
        operator: "set".to_string(),
        set_values: Some(values.iter().map(|value| (*value).to_string()).collect()),
        ..ScopeTermSpec::default()
    }
}

fn range_term(dimension: &str, start: &str, end: &str) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: dimension.to_string(),
        operator: "range".to_string(),
        range_start: Some(start.to_string()),
        range_end: Some(end.to_string()),
        ..ScopeTermSpec::default()
    }
}

fn git_reachable(dimension: &str, oid: &str) -> ScopeTermSpec {
    ScopeTermSpec {
        dimension: dimension.to_string(),
        operator: "git_reachable".to_string(),
        git_oid: Some(oid.to_string()),
        ..ScopeTermSpec::default()
    }
}

struct CompleteOracle;

impl GraphOracle for CompleteOracle {
    fn is_ancestor_or_equal(&self, ancestor: &str, descendant: &str) -> Option<bool> {
        Some(ancestor <= descendant)
    }
}

fn eight_term_specs() -> [ScopeTermSpec; 8] {
    [
        exact("domain", "code"),
        exact("project", "eidnara"),
        exact("entity", "store"),
        exact("branch", "main"),
        exact("environment", "test"),
        exact("region", "local"),
        version_range("deployment", ">=1.0.0, <3.0.0"),
        version_range("platform", ">=1.70.0, <2.0.0"),
    ]
}

fn algebra_benches(c: &mut Criterion) {
    let one = CanonicalScope::from_term_specs(&[exact("branch", "main")]).unwrap();
    let eight_specs = eight_term_specs();
    let eight = CanonicalScope::from_term_specs(&eight_specs).unwrap();
    let mut context = ScopeMatchContext::new();
    for (dimension, value) in [
        (Dimension::Domain, "code"),
        (Dimension::Project, "eidnara"),
        (Dimension::Entity, "store"),
        (Dimension::Branch, "main"),
        (Dimension::Environment, "test"),
        (Dimension::Region, "local"),
        (Dimension::Deployment, "2.0.0"),
        (Dimension::Platform, "1.98.0"),
    ] {
        context = context.with_value(dimension, value);
    }
    let first_miss = context
        .clone()
        .with_value(Dimension::Domain, "documentation");
    let outer_version =
        CanonicalScope::from_term_specs(&[version_range("platform", ">=1.0.0, <3.0.0")]).unwrap();
    let inner_version =
        CanonicalScope::from_term_specs(&[version_range("platform", ">=1.5.0, <2.0.0")]).unwrap();
    let exact_version = CanonicalScope::from_term_specs(&[exact("platform", "1.5.0")]).unwrap();
    let set_range = CanonicalScope::from_term_specs(&[
        set_term("branch", &["main", "release"]),
        set_term("environment", &["test", "prod"]),
        range_term("region", "a", "z"),
        range_term("project", "a", "z"),
    ])
    .unwrap();
    let ancestor = "11".repeat(20);
    let descendant = "22".repeat(20);
    let git = CanonicalScope::from_term_specs(&[git_reachable("branch", &ancestor)]).unwrap();
    let git_context = context.clone().with_head_commit(descendant);

    let mut group = c.benchmark_group("algebra");
    group.sample_size(100);
    for (name, scope) in [("one-term", &one), ("eight-term", &eight)] {
        group.bench_with_input(BenchmarkId::new("matches", name), scope, |b, scope| {
            b.iter(|| scope_matches(black_box(scope), black_box(&context), &UnknownGraph));
        });
        group.bench_with_input(BenchmarkId::new("subsumes", name), scope, |b, scope| {
            b.iter(|| scope_subsumes(black_box(scope), black_box(scope), &UnknownGraph));
        });
        group.bench_with_input(BenchmarkId::new("overlaps", name), scope, |b, scope| {
            b.iter(|| scope_overlaps(black_box(scope), black_box(scope), &UnknownGraph));
        });
    }
    group.bench_function("matches/eight-term-first-miss", |b| {
        b.iter(|| scope_matches(black_box(&eight), black_box(&first_miss), &UnknownGraph));
    });
    group.bench_function("subsumes/version-nested", |b| {
        b.iter(|| {
            scope_subsumes(
                black_box(&outer_version),
                black_box(&inner_version),
                &UnknownGraph,
            )
        });
    });
    group.bench_function("subsumes/version-exact", |b| {
        b.iter(|| {
            scope_subsumes(
                black_box(&outer_version),
                black_box(&exact_version),
                &UnknownGraph,
            )
        });
    });
    group.bench_function("overlaps/version-exact", |b| {
        b.iter(|| {
            scope_overlaps(
                black_box(&outer_version),
                black_box(&exact_version),
                &UnknownGraph,
            )
        });
    });
    group.bench_function("matches/set-range", |b| {
        b.iter(|| scope_matches(black_box(&set_range), black_box(&context), &UnknownGraph));
    });
    group.bench_function("subsumes/set-range", |b| {
        b.iter(|| scope_subsumes(black_box(&set_range), black_box(&set_range), &UnknownGraph));
    });
    group.bench_function("overlaps/set-range", |b| {
        b.iter(|| scope_overlaps(black_box(&set_range), black_box(&set_range), &UnknownGraph));
    });
    group.bench_function("matches/git", |b| {
        b.iter(|| scope_matches(black_box(&git), black_box(&git_context), &CompleteOracle));
    });
    group.bench_function("subsumes/git", |b| {
        b.iter(|| scope_subsumes(black_box(&git), black_box(&git), &CompleteOracle));
    });
    group.bench_function("overlaps/git", |b| {
        b.iter(|| scope_overlaps(black_box(&git), black_box(&git), &CompleteOracle));
    });
    group.bench_function("decode/eight-term", |b| {
        b.iter(|| CanonicalScope::from_term_specs(black_box(&eight_specs)).unwrap());
    });
    group.finish();
}

struct HistoryFixture {
    _dir: tempfile::TempDir,
    fixture: FixtureRepo,
    commits: Vec<gix::ObjectId>,
    foreign: gix::ObjectId,
}

fn linear_history(size: usize) -> HistoryFixture {
    let dir = tempfile::tempdir().unwrap();
    let fixture = init_repo(dir.path());
    let tree = write_tree(&fixture.repo, &[("f", "content")]);
    let mut commits = Vec::with_capacity(size);
    for index in 0..size {
        let parents = commits.last().copied().into_iter().collect::<Vec<_>>();
        commits.push(commit_tree(
            &fixture.repo,
            "main",
            &parents,
            tree,
            &format!("commit {index}"),
            i64::try_from(index + 1).unwrap(),
        ));
    }
    let foreign = commit_tree(&fixture.repo, "foreign", &[], tree, "foreign", 1_000_000);
    HistoryFixture {
        _dir: dir,
        fixture,
        commits,
        foreign,
    }
}

fn merge_history(size: usize) -> HistoryFixture {
    let dir = tempfile::tempdir().unwrap();
    let fixture = init_repo(dir.path());
    let tree = write_tree(&fixture.repo, &[("f", "content")]);
    let root = commit_tree(&fixture.repo, "node-0", &[], tree, "root", 1);
    let mut commits = vec![root];
    for index in 1..size {
        let first = commits[index - 1];
        let parents = if index > 2 && index % 3 == 0 {
            vec![first, commits[index / 3]]
        } else {
            vec![first]
        };
        commits.push(commit_tree(
            &fixture.repo,
            &format!("node-{index}"),
            &parents,
            tree,
            &format!("commit {index}"),
            i64::try_from(index + 1).unwrap(),
        ));
    }
    let foreign = commit_tree(&fixture.repo, "foreign", &[], tree, "foreign", 1_000_000);
    HistoryFixture {
        _dir: dir,
        fixture,
        commits,
        foreign,
    }
}

fn ancestry_benches(c: &mut Criterion) {
    let histories = [
        ("linear-100", linear_history(100)),
        ("linear-10k", linear_history(10_000)),
        ("merge-5k", merge_history(5_000)),
    ];
    let mut group = c.benchmark_group("ancestry");
    group.sample_size(20);
    for (shape, history) in &histories {
        let snapshot = checkout(&history.fixture, *history.commits.last().unwrap());
        let tip = history.commits.last().unwrap().to_string();
        let budget = EvalBudget::unbounded();
        for (mix, ancestor) in [
            (
                "near",
                history.commits[history.commits.len().saturating_sub(10)].to_string(),
            ),
            ("far", history.commits[0].to_string()),
            ("unrelated", history.foreign.to_string()),
        ] {
            group.bench_function(BenchmarkId::new(*shape, mix), |b| {
                b.iter(|| {
                    ResolutionLadder::new(&snapshot, &budget)
                        .is_ancestor_or_equal(black_box(&ancestor), black_box(&tip))
                });
            });
        }
        let ladder = ResolutionLadder::new(&snapshot, &budget);
        let ancestor = history.commits[0].to_string();
        let _ = ladder.is_ancestor_or_equal(&ancestor, &tip);
        group.bench_function(BenchmarkId::new(*shape, "memo-hit"), |b| {
            b.iter(|| ladder.is_ancestor_or_equal(black_box(&ancestor), black_box(&tip)));
        });
        if *shape == "linear-10k" {
            let ancestors: Vec<String> = [0, 2_500, 5_000, 7_500]
                .map(|index| history.commits[index].to_string())
                .into();
            group.bench_function(BenchmarkId::new(*shape, "distinct-4"), |b| {
                b.iter(|| {
                    let ladder = ResolutionLadder::new(&snapshot, &budget);
                    for ancestor in &ancestors {
                        black_box(
                            ladder.is_ancestor_or_equal(black_box(ancestor), black_box(&tip)),
                        );
                    }
                });
            });
        }
    }
    group.finish();
}

fn snapshot_matrix_fixture(modified: usize, untracked: usize) -> (tempfile::TempDir, FixtureRepo) {
    let dir = tempfile::tempdir().unwrap();
    let fixture = init_repo(dir.path());
    let names: Vec<_> = (0..1_200)
        .map(|i| format!("dir{}/f{i:04}.txt", i % 8))
        .collect();
    let contents: Vec<_> = (0..1_200).map(|i| format!("content {i}\n")).collect();
    let files: Vec<_> = names
        .iter()
        .map(String::as_str)
        .zip(contents.iter().map(String::as_str))
        .collect();
    let head = commit_snapshot(&fixture.repo, "main", &[], &files, "base", 1);
    set_head(&fixture.repo, "main");
    materialize(&fixture.repo, head);
    for name in names.iter().take(modified) {
        write_worktree_file(&fixture.repo, name, "edited\n");
    }
    for i in 0..untracked {
        write_worktree_file(
            &fixture.repo,
            &format!("new{}/u{i:04}.txt", i % 8),
            "untracked\n",
        );
    }
    (dir, fixture)
}

fn snapshot_matrix_benches(c: &mut Criterion) {
    let fixtures = [
        ("clean", snapshot_matrix_fixture(0, 0)),
        ("dirty-10", snapshot_matrix_fixture(10, 0)),
        ("dirty-1000", snapshot_matrix_fixture(1_000, 0)),
        ("untracked-1000", snapshot_matrix_fixture(0, 1_000)),
    ];
    let mut group = c.benchmark_group("snapshot_matrix");
    group.sample_size(10);
    let budget = EvalBudget::unbounded();
    for (name, (_dir, fixture)) in &fixtures {
        group.bench_function(*name, |b| {
            b.iter(|| snapshot_checkout(black_box(&fixture.root), &budget).unwrap());
        });
    }
    group.finish();
}

fn payload_decode_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("payload_decode");
    group.sample_size(100);
    for size in [0usize, 4, 16, 64] {
        let affected = (0..size).map(|i| format!("src/mod{i}/file.rs")).collect();
        let checks = (0..size)
            .map(|i| CheckSpec::ConfigKey {
                path: format!("configs/app{i}.toml"),
                key: format!("key_{i}"),
            })
            .collect();
        let payload = ObjectApplicabilitySpec::new(affected, checks).encode();
        group.bench_with_input(BenchmarkId::new("decode", size), &payload, |b, payload| {
            b.iter(|| black_box(ObjectApplicabilitySpec::decode(Some(black_box(payload)))));
        });
    }
    group.finish();
}

fn cheap_check_benches(c: &mut Criterion) {
    let (_dir, fixture, _base, tip) = applicability_fixture();
    let snapshot = checkout(&fixture, tip);
    write_worktree_file(
        &fixture.repo,
        "config.toml",
        "feature_flag = true\nother = 1\n",
    );
    let budget = EvalBudget::unbounded();
    let checks = [
        (
            "file-hit",
            CheckSpec::FileExists {
                path: "src/lib.rs".to_string(),
            },
        ),
        (
            "file-miss",
            CheckSpec::FileExists {
                path: "absent.txt".to_string(),
            },
        ),
        (
            "config-key",
            CheckSpec::ConfigKey {
                path: "config.toml".to_string(),
                key: "feature_flag".to_string(),
            },
        ),
    ];
    let mut group = c.benchmark_group("cheap_checks");
    for (name, check) in &checks {
        group.bench_function(BenchmarkId::new("run_cheap_check", name), |b| {
            b.iter_batched(
                CheckCache::new,
                |mut cache| run_cheap_check(&snapshot, black_box(check), &budget, &mut cache),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn applicability_fixture() -> (tempfile::TempDir, FixtureRepo, gix::ObjectId, gix::ObjectId) {
    let dir = tempfile::tempdir().unwrap();
    let fixture = init_repo(dir.path());
    let base = commit_snapshot(
        &fixture.repo,
        "main",
        &[],
        &[("src/lib.rs", "pub fn a() {}\n")],
        "base",
        1,
    );
    let tip = commit_snapshot(
        &fixture.repo,
        "main",
        &[base],
        &[("src/lib.rs", "pub fn a() {}\npub fn b() {}\n")],
        "tip",
        2,
    );
    (dir, fixture, base, tip)
}

fn candidates(
    fixture: &FixtureRepo,
    base: gix::ObjectId,
    size: usize,
    anchored: bool,
) -> Vec<ApplicabilityCandidate> {
    let anchor = anchored.then(|| reachable_anchor(fixture, "shared", base));
    (0..size)
        .map(|index| ApplicabilityCandidate {
            anchor: anchor.clone(),
            payload: Some(ObjectApplicabilitySpec::new(vec![], vec![]).encode()),
            ..candidate(&format!("object-{index}"))
        })
        .collect()
}

fn batch_benches(c: &mut Criterion) {
    let (_dir, fixture, base, tip) = applicability_fixture();
    let snapshot = checkout(&fixture, tip);
    let query = QueryContext::default();
    let scope = ScopeMatchContext::new();
    let budget = EvalBudget::unbounded();
    let mut group = c.benchmark_group("batch");
    group.sample_size(20);
    for size in [1, 8, 64, 512] {
        for anchored in [false, true] {
            let candidates = candidates(&fixture, base, size, anchored);
            let name = if anchored { "anchored" } else { "plain" };
            group.bench_function(BenchmarkId::new(format!("cold-{name}"), size), |b| {
                b.iter_batched(
                    ApplicabilityEngine::new,
                    |engine| {
                        engine.evaluate_batch(
                            black_box(&snapshot),
                            black_box(&query),
                            black_box(&scope),
                            black_box(&candidates),
                            &budget,
                        )
                    },
                    BatchSize::SmallInput,
                );
            });
            let engine = ApplicabilityEngine::new();
            let _ = engine.evaluate_batch(&snapshot, &query, &scope, &candidates, &budget);
            group.bench_function(BenchmarkId::new(format!("warm-{name}"), size), |b| {
                b.iter(|| {
                    engine.evaluate_batch(
                        black_box(&snapshot),
                        black_box(&query),
                        black_box(&scope),
                        black_box(&candidates),
                        &budget,
                    )
                });
            });
        }
    }

    for index in 0..1_000 {
        write_worktree_file(&fixture.repo, &format!("dirty/{index}.txt"), "uncommitted");
    }
    let dirty_snapshot = snapshot_checkout(&fixture.root, &budget).unwrap();
    for (name, affected_path) in [("dirty-disjoint", "docs"), ("dirty-overlap", "dirty")] {
        let candidates: Vec<_> = (0..512)
            .map(|index| ApplicabilityCandidate {
                payload: Some(
                    ObjectApplicabilitySpec::new(vec![affected_path.to_string()], vec![]).encode(),
                ),
                ..candidate(&format!("dirty-object-{index}"))
            })
            .collect();
        group.bench_function(BenchmarkId::new(name, 512), |b| {
            b.iter_batched(
                ApplicabilityEngine::new,
                |engine| {
                    engine.evaluate_batch(
                        black_box(&dirty_snapshot),
                        black_box(&query),
                        black_box(&scope),
                        black_box(&candidates),
                        &budget,
                    )
                },
                BatchSize::SmallInput,
            );
        });
    }

    let scoped_context = ScopeMatchContext::new()
        .with_value(Dimension::Domain, "code")
        .with_value(Dimension::Project, "eidnara")
        .with_value(Dimension::Entity, "store")
        .with_value(Dimension::Branch, "main")
        .with_value(Dimension::Environment, "test")
        .with_value(Dimension::Region, "local")
        .with_value(Dimension::Deployment, "2.0.0")
        .with_value(Dimension::Platform, "1.98.0");
    let scoped_candidates: Vec<_> = (0..512)
        .map(|index| ApplicabilityCandidate {
            scope_terms: Some(eight_term_specs().to_vec()),
            ..candidate(&format!("scoped-object-{index}"))
        })
        .collect();
    group.bench_function(BenchmarkId::new("cold-scoped", 512), |b| {
        b.iter_batched(
            ApplicabilityEngine::new,
            |engine| {
                engine.evaluate_batch(
                    black_box(&snapshot),
                    black_box(&query),
                    black_box(&scoped_context),
                    black_box(&scoped_candidates),
                    &budget,
                )
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn anchor_density_benches(c: &mut Criterion) {
    let (_dir, fixture, base, tip) = applicability_fixture();
    let snapshot = checkout(&fixture, tip);
    let query = QueryContext::default();
    let scope = ScopeMatchContext::new();
    let budget = EvalBudget::unbounded();
    let anchor = reachable_anchor(&fixture, "shared", base);
    let mut group = c.benchmark_group("anchor_density");
    for density in ["none", "sparse", "every"] {
        let candidates: Vec<_> = (0..64)
            .map(|index| ApplicabilityCandidate {
                anchor: match density {
                    "every" => Some(anchor.clone()),
                    "sparse" if index % 8 == 0 => Some(anchor.clone()),
                    _ => None,
                },
                ..candidate(&format!("density-{index}"))
            })
            .collect();
        group.bench_function(BenchmarkId::new("cold", density), |b| {
            b.iter_batched(
                ApplicabilityEngine::new,
                |engine| engine.evaluate_batch(&snapshot, &query, &scope, &candidates, &budget),
                BatchSize::SmallInput,
            );
        });
        let engine = ApplicabilityEngine::new();
        let _ = engine.evaluate_batch(&snapshot, &query, &scope, &candidates, &budget);
        group.bench_function(BenchmarkId::new("warm", density), |b| {
            b.iter(|| engine.evaluate_batch(&snapshot, &query, &scope, &candidates, &budget));
        });
    }
    group.finish();
}

fn payload_check_benches(c: &mut Criterion) {
    let (_dir, fixture, _base, tip) = applicability_fixture();
    let snapshot = checkout(&fixture, tip);
    let query = QueryContext::default();
    let scope = ScopeMatchContext::new();
    let budget = EvalBudget::unbounded();
    let mut group = c.benchmark_group("payload_checks");
    for count in [0usize, 4, 16] {
        let checks = (0..count)
            .map(|_| CheckSpec::FileExists {
                path: "src/lib.rs".to_string(),
            })
            .collect();
        let payload = ObjectApplicabilitySpec::new(vec![], checks).encode();
        let candidates: Vec<_> = (0..64)
            .map(|index| ApplicabilityCandidate {
                payload: Some(payload.clone()),
                ..candidate(&format!("checked-{index}"))
            })
            .collect();
        group.bench_function(BenchmarkId::new("cold64", count), |b| {
            b.iter_batched(
                ApplicabilityEngine::new,
                |engine| engine.evaluate_batch(&snapshot, &query, &scope, &candidates, &budget),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn staleness_benches(c: &mut Criterion) {
    let (_dir, fixture, _base, tip) = applicability_fixture();
    let first = checkout(&fixture, tip);
    let query = QueryContext::default();
    let scope = ScopeMatchContext::new();
    let budget = EvalBudget::unbounded();
    let candidates = candidates(&fixture, tip, 64, false);
    let engine = ApplicabilityEngine::new();
    let _ = engine.evaluate_batch(&first, &query, &scope, &candidates, &budget);
    let mut edit = 0u64;
    let mut group = c.benchmark_group("staleness");
    group.bench_function("warm-new-snapshot/64", |b| {
        b.iter_batched(
            || {
                edit += 1;
                write_worktree_file(&fixture.repo, "moving.txt", &format!("snapshot {edit}\n"));
                snapshot_checkout(&fixture.root, &budget).unwrap()
            },
            |snapshot| engine.evaluate_batch(&snapshot, &query, &scope, &candidates, &budget),
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn adversarial_benches(c: &mut Criterion) {
    let (_dir, fixture, base, tip) = applicability_fixture();
    let snapshot = checkout(&fixture, tip);
    let query = QueryContext::default();
    let scope = ScopeMatchContext::new();
    let budget = EvalBudget::unbounded();
    let anchor = reachable_anchor(&fixture, "template", base);
    let distinct_anchors: Vec<_> = (0..512)
        .map(|index| {
            let mut anchor = anchor.clone();
            anchor.anchor_id = format!("anchor-{index}");
            ApplicabilityCandidate {
                anchor: Some(anchor),
                ..candidate(&format!("anchored-{index}"))
            }
        })
        .collect();
    let affected = (0..64).map(|i| format!("generated/{i}")).collect();
    let payload = ObjectApplicabilitySpec::new(affected, vec![]).encode();
    let huge_paths: Vec<_> = (0..512)
        .map(|index| ApplicabilityCandidate {
            payload: Some(payload.clone()),
            ..candidate(&format!("paths-{index}"))
        })
        .collect();
    let malformed_versions: Vec<_> = (0..512)
        .map(|index| ApplicabilityCandidate {
            scope_terms: Some(vec![version_range("platform", ">=not-a-version")]),
            ..candidate(&format!("malformed-{index}"))
        })
        .collect();
    let payload_a =
        ObjectApplicabilitySpec::new((0..64).map(|i| format!("alpha/{i}")).collect(), vec![])
            .encode();
    let payload_b =
        ObjectApplicabilitySpec::new((0..64).map(|i| format!("beta/{i}")).collect(), vec![])
            .encode();
    let alternating_payloads: Vec<_> = (0..512)
        .map(|index| ApplicabilityCandidate {
            payload: Some(if index % 2 == 0 {
                payload_a.clone()
            } else {
                payload_b.clone()
            }),
            ..candidate(&format!("payload-{index}"))
        })
        .collect();
    let mut group = c.benchmark_group("adversarial");
    for (name, candidates) in [
        ("distinct-anchors", &distinct_anchors),
        ("affected-paths-64", &huge_paths),
        ("malformed-version", &malformed_versions),
        ("nonadjacent-payloads-64", &alternating_payloads),
    ] {
        group.bench_function(name, |b| {
            b.iter_batched(
                ApplicabilityEngine::new,
                |engine| engine.evaluate_batch(&snapshot, &query, &scope, candidates, &budget),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn configure() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
}

criterion_group! {
    name = benches;
    config = configure();
    targets = algebra_benches, ancestry_benches, snapshot_matrix_benches, payload_decode_benches, cheap_check_benches, batch_benches, anchor_density_benches, payload_check_benches, staleness_benches, adversarial_benches
}

fn main() {
    // `cargo test --all-targets` omits `--bench`, so the binary returns before fixture setup writes thousands of gix commits and worktree files. commentlint: allow(JUDGE)
    // `cargo bench -- --test` keeps `--bench`, so Criterion runs every group once. commentlint: allow(JUDGE)
    if !std::env::args().any(|arg| arg == "--bench") {
        eprintln!("scope_algebra: fixture setup runs only under `cargo bench`");
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
